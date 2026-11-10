//! A workspace watch belongs to its owning tree level or document tab.

use super::*;

#[derive(Clone, Copy, serde::Serialize)]
struct WatchKey {
    shell: u64,
    id: u64,
}

pub(super) struct WorkspaceWatch {
    key: WatchKey,
    requested: std::cell::Cell<bool>,
}

impl WorkspaceWatch {
    pub(super) fn new(shell: u64, id: u64) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self {
            key: WatchKey { shell, id },
            requested: std::cell::Cell::new(false),
        })
    }

    pub(super) fn id(&self) -> u64 {
        self.key.id
    }

    pub(super) async fn follow(
        self: &std::rc::Rc<Self>,
        command: &str,
        mut args: Value,
    ) -> Option<Result<Value, String>> {
        // The caller holds a lease until registration settles. If the owner
        // closes while IPC is in flight, dropping that last lease unwatches it.
        if std::rc::Rc::strong_count(self) == 1 {
            return None;
        }
        self.requested.set(true);
        args["key"] = json!(self.key);
        let reply = bridge::invoke(command, args).await;
        (std::rc::Rc::strong_count(self) > 1).then_some(reply)
    }
}

impl Drop for WorkspaceWatch {
    fn drop(&mut self) {
        if self.requested.get() {
            let key = self.key;
            wasm_bindgen_futures::spawn_local(async move {
                let _ = bridge::invoke("unwatch_workspace_path", json!({"key": key})).await;
            });
        }
    }
}

impl DshApp {
    pub(super) fn workspace_watch_id(&self, event: &Value) -> Option<u64> {
        let shell = event.get("shell")?.as_u64()?;
        (self.browser_shell == Some(shell)).then_some(())?;
        event.get("watchId")?.as_u64()
    }
}
