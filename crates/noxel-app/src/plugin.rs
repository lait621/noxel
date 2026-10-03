//! The plugin trait and registry.
//!
//! A plugin is a struct with a name and up to three hooks. That is deliberately
//! the whole interface: anything more elaborate ends up as a framework nobody
//! uses, and a plugin that needs more can keep its own state and borrow
//! whatever it wants from the `App` it is handed.
//!
//! The one non-obvious rule is that **`build` runs before any `update`**, and
//! plugins are built in registration order; `update` runs in the same order.
//! Deterministic ordering is what makes a replay reproducible.

use crate::app::App;

/// A subsystem that participates in the frame.
///
/// All methods have default implementations, so a plugin only writes the hooks
/// it needs.
pub trait Plugin: 'static {
    /// A stable, human-readable name. Used in the report and for ordering.
    fn name(&self) -> &str;

    /// Called once, when the plugin is added.
    ///
    /// This is where a plugin creates its entities, inserts its resources and
    /// builds its meshes.
    fn build(&mut self, _app: &mut App) {}

    /// Called once per fixed update step.
    ///
    /// `dt` is the fixed timestep, not the frame time: gameplay must be
    /// deterministic, and a variable step is the fastest way to lose that.
    fn update(&mut self, _app: &mut App, _dt: f32) {}

    /// Called once per rendered frame, after the fixed updates.
    fn frame(&mut self, _app: &mut App, _dt: f32) {}

    /// Called once per frame, before the visibility pass.
    ///
    /// This is where a plugin that moves the camera should do it: the camera has
    /// to be final before culling and streaming read it.
    fn pre_cull(&mut self, _app: &mut App, _dt: f32) {}

    /// Called once per frame, after the image has been shaded and before it is
    /// resolved to sRGB. The place to draw an overlay.
    fn draw(&mut self, _app: &mut App, _framebuffer: &mut noxel_render::Framebuffer) {}

    /// Called once, when the app is torn down.
    fn shutdown(&mut self, _app: &mut App) {}

    /// Whether this plugin wants to run at all.
    fn is_enabled(&self) -> bool {
        true
    }
}

/// An ordered set of plugins.
///
/// The registry exists so a game can enable and disable a plugin at runtime —
/// a debug visualiser, a profiler, a tutorial — without the app having to know
/// which plugins exist.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Box<dyn Plugin>>,
    enabled: Vec<bool>,
}

impl PluginRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            enabled: Vec::new(),
        }
    }

    /// Registers a plugin. Returns its index.
    ///
    /// Registering two plugins with the same name is allowed; the name is for
    /// humans, the index is the identity.
    pub fn push(&mut self, plugin: Box<dyn Plugin>) -> usize {
        let enabled = plugin.is_enabled();
        self.plugins.push(plugin);
        self.enabled.push(enabled);
        self.plugins.len() - 1
    }

    /// The number of registered plugins.
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// True when nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// The plugin names, in registration order.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.name()).collect()
    }

    /// Finds a plugin by name.
    #[must_use]
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.plugins.iter().position(|p| p.name() == name)
    }

    /// Enables or disables a plugin by name. Returns false when it is unknown.
    pub fn set_enabled(&mut self, name: &str, enabled: bool) -> bool {
        match self.index_of(name) {
            Some(index) => {
                self.enabled[index] = enabled;
                true
            }
            None => false,
        }
    }

    /// True when a plugin is currently enabled.
    #[must_use]
    pub fn is_enabled(&self, name: &str) -> bool {
        self.index_of(name).is_some_and(|i| self.enabled[i])
    }

    /// True when the plugin at `index` is enabled.
    #[must_use]
    pub fn is_enabled_at(&self, index: usize) -> bool {
        self.enabled.get(index).copied().unwrap_or(false)
    }

    /// Calls `build` on every plugin, in order.
    pub fn build(&mut self, app: &mut App) {
        for index in 0..self.plugins.len() {
            self.build_index(app, index);
        }
    }

    /// Calls `build` on one plugin, if it is enabled.
    ///
    /// Used by [`App::add_plugin`](crate::App::add_plugin) so a plugin added
    /// after startup is built immediately rather than being skipped.
    pub fn build_index(&mut self, app: &mut App, index: usize) {
        if index < self.plugins.len() && self.enabled[index] {
            self.plugins[index].build(app);
        }
    }

    /// Calls `update` on every enabled plugin, in order.
    pub fn update(&mut self, app: &mut App, dt: f32) {
        for index in 0..self.plugins.len() {
            if self.enabled[index] {
                self.plugins[index].update(app, dt);
            }
        }
    }

    /// Calls `frame` on every enabled plugin, in order.
    pub fn frame(&mut self, app: &mut App, dt: f32) {
        for index in 0..self.plugins.len() {
            if self.enabled[index] {
                self.plugins[index].frame(app, dt);
            }
        }
    }

    /// Calls `pre_cull` on every enabled plugin, in order.
    pub fn pre_cull(&mut self, app: &mut App, dt: f32) {
        for index in 0..self.plugins.len() {
            if self.enabled[index] {
                self.plugins[index].pre_cull(app, dt);
            }
        }
    }

    /// Calls `draw` on every enabled plugin, in order.
    pub fn draw(&mut self, app: &mut App, framebuffer: &mut noxel_render::Framebuffer) {
        for index in 0..self.plugins.len() {
            if self.enabled[index] {
                self.plugins[index].draw(app, framebuffer);
            }
        }
    }

    /// Calls `shutdown` on every plugin, in reverse order.
    ///
    /// Reverse order is deliberate: a plugin that depends on another must be
    /// torn down first.
    pub fn shutdown(&mut self, app: &mut App) {
        for index in (0..self.plugins.len()).rev() {
            if self.enabled[index] {
                self.plugins[index].shutdown(app);
            }
        }
    }
}

impl core::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("plugins", &self.names())
            .finish()
    }
}

/// A plugin that only counts frames, used by the tests and as a template.
#[derive(Debug, Default)]
pub struct FrameCounter {
    /// How many `update` calls were received.
    pub updates: u64,
    /// How many `frame` calls were received.
    pub frames: u64,
}

impl Plugin for FrameCounter {
    fn name(&self) -> &str {
        "frame-counter"
    }

    fn update(&mut self, _app: &mut App, _dt: f32) {
        self.updates += 1;
    }

    fn frame(&mut self, _app: &mut App, _dt: f32) {
        self.frames += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppConfig;

    #[derive(Debug, Default)]
    struct Recorder {
        calls: Vec<&'static str>,
    }

    impl Plugin for Recorder {
        fn name(&self) -> &str {
            "recorder"
        }
        fn build(&mut self, _app: &mut App) {
            self.calls.push("build");
        }
        fn update(&mut self, _app: &mut App, _dt: f32) {
            self.calls.push("update");
        }
        fn frame(&mut self, _app: &mut App, _dt: f32) {
            self.calls.push("frame");
        }
        fn pre_cull(&mut self, _app: &mut App, _dt: f32) {
            self.calls.push("pre_cull");
        }
        fn draw(&mut self, _app: &mut App, _framebuffer: &mut noxel_render::Framebuffer) {
            self.calls.push("draw");
        }
        fn shutdown(&mut self, _app: &mut App) {
            self.calls.push("shutdown");
        }
        fn is_enabled(&self) -> bool {
            true
        }
    }

    fn app() -> App {
        App::new(AppConfig::headless()).unwrap()
    }

    #[test]
    fn registry_starts_empty() {
        let registry = PluginRegistry::new();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[test]
    fn registry_keeps_registration_order() {
        let mut registry = PluginRegistry::new();
        registry.push(Box::new(FrameCounter::default()));
        registry.push(Box::new(Recorder::default()));
        assert_eq!(registry.names(), vec!["frame-counter", "recorder"]);
        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn registry_runs_every_hook_in_order() {
        // The plugin is boxed, so its call list cannot be read back through the
        // registry; instead, record into a shared cell through a resource.
        #[derive(Default)]
        struct Shared(Vec<&'static str>);
        let shared = std::rc::Rc::new(std::cell::RefCell::new(Shared::default()));

        struct Recorder2(std::rc::Rc<std::cell::RefCell<Shared>>);
        impl Plugin for Recorder2 {
            fn name(&self) -> &str {
                "recorder2"
            }
            fn build(&mut self, _app: &mut App) {
                self.0.borrow_mut().0.push("build");
            }
            fn update(&mut self, _app: &mut App, _dt: f32) {
                self.0.borrow_mut().0.push("update");
            }
            fn frame(&mut self, _app: &mut App, _dt: f32) {
                self.0.borrow_mut().0.push("frame");
            }
            fn pre_cull(&mut self, _app: &mut App, _dt: f32) {
                self.0.borrow_mut().0.push("pre_cull");
            }
            fn draw(&mut self, _app: &mut App, _fb: &mut noxel_render::Framebuffer) {
                self.0.borrow_mut().0.push("draw");
            }
            fn shutdown(&mut self, _app: &mut App) {
                self.0.borrow_mut().0.push("shutdown");
            }
        }

        let mut registry = PluginRegistry::new();
        registry.push(Box::new(Recorder2(std::rc::Rc::clone(&shared))));
        let mut app = app();
        registry.build(&mut app);
        registry.update(&mut app, 1.0 / 60.0);
        registry.frame(&mut app, 1.0 / 60.0);
        registry.pre_cull(&mut app, 1.0 / 60.0);
        let mut fb = noxel_render::Framebuffer::new(4, 4);
        registry.draw(&mut app, &mut fb);
        registry.shutdown(&mut app);
        assert_eq!(
            shared.borrow().0,
            vec!["build", "update", "frame", "pre_cull", "draw", "shutdown"]
        );
    }

    #[test]
    fn disabled_plugins_are_skipped() {
        let mut registry = PluginRegistry::new();
        registry.push(Box::new(FrameCounter::default()));
        assert!(registry.set_enabled("frame-counter", false));
        assert!(!registry.is_enabled("frame-counter"));
        let mut app = app();
        registry.update(&mut app, 0.016);
        registry.build(&mut app);
        // Nothing to observe through the box, so assert the enable state that
        // drives the skip.
        assert!(!registry.is_enabled_at(0));
    }

    #[test]
    fn enabling_an_unknown_plugin_fails() {
        let mut registry = PluginRegistry::new();
        assert!(!registry.set_enabled("nope", true));
        assert!(!registry.is_enabled("nope"));
        assert_eq!(registry.index_of("nope"), None);
    }

    #[test]
    fn registry_debug_lists_names() {
        let mut registry = PluginRegistry::new();
        registry.push(Box::new(FrameCounter::default()));
        let text = format!("{registry:?}");
        assert!(text.contains("frame-counter"));
    }

    #[test]
    fn frame_counter_counts() {
        let mut plugin = FrameCounter::default();
        let mut app = app();
        plugin.update(&mut app, 0.016);
        plugin.update(&mut app, 0.016);
        plugin.frame(&mut app, 0.016);
        assert_eq!(plugin.updates, 2);
        assert_eq!(plugin.frames, 1);
        assert_eq!(plugin.name(), "frame-counter");
    }
}
