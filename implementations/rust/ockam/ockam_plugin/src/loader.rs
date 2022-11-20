use crate::PluginAPI;
use std::mem;
use std::sync::{Arc, Mutex, Once};

//keep plugins private to force load_plugins() usage
static PLUGINS: Mutex<Vec<Arc<dyn PluginAPI>>> = Mutex::new(vec![]);
static LOAD_PLUGINS: Once = Once::new();

/**
    Loads & returns all plugins within plugins/ directory.
    Thread-safe
*/
pub fn load_plugins() -> &'static Mutex<Vec<Arc<dyn PluginAPI>>> {
    LOAD_PLUGINS.call_once(|| {
        let plugin_api;
        unsafe {
            let lib = libloading::Library::new(
                "/home/davide/workspace/github/build-trust/ockam/target/debug/libockam_pong.so",
            )
            .expect("cannot load plugin");
            let func: libloading::Symbol<unsafe fn() -> Arc<dyn PluginAPI>> = lib
                .get(b"create_plugin_api")
                .expect("cannot find 'create_plugin_api' symbol");
            plugin_api = func();
            //if the library gets deleted the created reference won't be usable anymore
            mem::forget(lib);
        }
        PLUGINS.lock().unwrap().push(plugin_api);
    });

    &PLUGINS
}

pub fn find_plugin(name: &str) -> Option<Arc<dyn PluginAPI>> {
    load_plugins()
        .lock()
        .unwrap()
        .iter()
        .find(|plugin| plugin.name() == name)
        .map(|arc| arc.clone())
}
