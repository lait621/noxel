# ADR 0014 — Weather is a system, not an effect

**Status:** accepted
**Date:** 2026-10-03
**Related:** [0006](0006-deterministic-generation.md), [0013](0013-audio.md)

## Context

A game that wants rain usually gets a screen-space particle loop: spawn streaks
at random screen positions, move them down the screen, wrap them at the bottom.
It is twenty lines and it is always wrong in the same way, and the name of that
way is **world space**.

The loop produces rain that:

* does not move when the camera does, so walking makes the shower slide with you;
* has no depth, because every streak falls at one speed with one opacity, so it
  reads as a scrolling texture rather than as volume;
* never lands, so the ground is never wet and there is no splash — nothing in
  the frame says the rain is hitting anything;
* is drawn over the player and in front of buildings it should be behind.

Noxel Valley shipped exactly that loop, and the report was "the rain is too
fake, and it is just an animation playing on the screen". That is a precise
description of the bug.

## Decision

Weather is a **crate**, `noxel-weather`, that simulates precipitation in world
space and draws nothing.

* Every drop has a world position, a fall speed, a length and a **depth**.
* Drops are spawned in a volume around the camera rather than across the world:
  a farm is forty tiles across and only the screen is visible, so simulating the
  rest is work nobody sees.
* Drops that reach the ground become **splashes** with their own short life. Snow
  does not splash, because a white ring under every flake is the fastest way to
  make snow look like rain.
* Near drops are faster, longer and more opaque than far ones. That spread *is*
  the volume; without it the rain is a sheet.
* `Kind` × `intensity`, and the intensity **ramps** over seconds. Weather does
  not switch.
* Wind is a direction and a speed, and the slant of a drop is derived from it, so
  the wind that moves the rain is the wind you see.

The crate returns `&[Drop]` and `&[Splash]`. The game projects them through its
own camera, which is the only part that knows about pixels.

## Consequences

**The rain is slower than real rain.** A 270-pixel view is about seventeen tiles
wide, and rain at a true 9 m/s crosses it before the eye can follow a drop. The
speeds are tuned to what *reads* as rain at that scale, and the module says so
rather than pretending they are physical.

**A drop's length is its motion blur**, not a constant: it is the distance a drop
travels in one frame, plus a little. The first version used a constant in world
units and drew scratches a quarter of the screen long.

**`Drop` carries no velocity.** The renderer derives the streak direction from
the drop's speed and the current wind, which keeps the struct small and means a
wind change slants the rain already in the air.

## Alternatives rejected

**A screen-space effect owned by the game.** This is what it was. It is cheaper
and it cannot be made to look right, for the four reasons above.

**A 3D particle system with lighting and collision.** Enormously more machinery
for a top-down game whose rain only ever falls on flat ground. The volume,
layers and splashes are the parts that matter; the rest is not visible.

**Depth-of-field or a screen tint instead of particles.** Rejected as the primary
mechanism but kept as a modifier: the storm darkens the frame, and that is a
property of the kind rather than a second system.
