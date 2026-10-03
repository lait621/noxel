# ADR 0016 — Mesh lifetime, and the leak a rebuild hides

**Status:** accepted
**Date:** 2026-10-03
**Supersedes:** nothing
**Related:** [0003](0003-software-renderer-first.md), [08-performance](../08-performance.md), [field notes](../field-notes.md)

## Context

A `Scene` owns meshes and instances separately, and that separation is
deliberate: a prefab is one mesh drawn by many instances, and a streaming world
wants to remove the instance that streamed out without touching the geometry
that other chunks still share. `Scene::remove_instance` therefore keeps the mesh,
and the documentation says so.

What it did not say is what happens to a game that builds geometry **for one
instance** and rebuilds it. The obvious-looking loop is:

```rust
let handle = scene.add_mesh(mesh);
let instance = scene.spawn("farm.ground", handle, material, Transform::IDENTITY);
scene.remove_instance(previous);          // "I have replaced it"
```

That is not an update. It is `add_mesh` without a matching `remove_mesh`, so one
mesh — the entire vertex and index data of the farm, hundreds of kilobytes —
stays in the arena forever, per frame. Noxel Valley shipped this: a rebuild key
was written as the map revision instead of the key it was compared against, so
the guard never matched, the farm's three meshes were rebuilt **every frame**,
and the process grew from 13 MB to 850 MB in fourteen seconds and kept going.
Nothing failed. Nothing logged. The frame rate even looked fine, because the
work is only half a millisecond.

Three things were missing, and they are three different kinds of thing:

1. **An API for the common case.** There was no way to say "this mesh is this
   instance's, release them together", and no way to say "replace this mesh's
   geometry" without changing every handle.
2. **A number.** `SceneStats::memory_bytes` existed, and nothing recorded it
   per frame, so the growth was invisible on the very overlay a developer would
   have been staring at.
3. **A test.** The suite asserted that streaming does not grow memory
   (`memory_stays_bounded_over_a_long_walk`) and had no equivalent for the
   scene, which is where a procedural game's memory actually lives.

## Decision

**Give the scene an explicit lifetime for geometry, make its size visible every
frame, and test the shape of the mistake.**

* `Scene::replace_mesh(handle, mesh)` — rebuild in place. Every instance
  pointing at the handle keeps pointing at it, their cached bounds are
  refreshed, and no mesh is added. Returns the mesh back on a stale handle so
  the caller can fall back to `add_mesh` without losing the work.
* `Scene::despawn(instance)` — remove an instance **and its mesh**, unless
  another instance still references that mesh. The counterpart to `spawn`.
* `Scene::prune_unused_meshes()` — remove every mesh no instance references.
  A sledgehammer for a caller that knows what it means, and explicitly not
  automatic.
* `Scene::mesh_bytes()`, and `App` records `meshes` and `mesh mem KB` as debug
  counters on **every frame**, beside the chunk counters that were already
  there.
* `remove_instance` keeps its meaning, and its documentation now says what
  happens if you use it to replace geometry.

**Automatic pruning was rejected.** Freeing every unreferenced mesh at the end
of a frame looks like it would have prevented this bug, and it would break the
pattern the separation exists for: a game that builds a library of meshes and
spawns and despawns instances onto them as the player walks would have its
library collected out from under it on the first frame nothing used it. A silent
collection of live assets is a worse failure than a loud leak, and the leak is
now audible.

## Consequences

**The mistake is one line to fix and one call to prevent.** The map-revision
key is the game's bug; `despawn` instead of `remove_instance` is the game's
other one; and a scene that rebuilds in place cannot grow whatever the key says.

**Memory is on the overlay.** `meshes` and `mesh mem KB` climb in front of
whoever is looking at the screen, which is the diagnostic that was missing.
Two counters are a rounding error in cost (one pass over the mesh list) and the
difference between "the game is slow" and "the game is leaking".

**The invariant is testable.** `scene::tests` now contains the leak as a
*passing test* — 100 rebuilds through `add_mesh` + `remove_instance` keep 101
meshes, and the same 100 rebuilds through `replace_mesh` keep 1 — so the trap is
written down where the next person will hit it, rather than in a commit message.

**Meshes are still not reference-counted.** A `MeshHandle` with a use count
would have caught this too, and would have made every `spawn` and every
`remove_instance` in the engine pay for a case that a handful of call sites can
state explicitly. The engine's rule is that ownership is stated, not inferred;
this keeps it.
