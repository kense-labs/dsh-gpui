//! Host terminal emulator using the same xterm and serializer as DeepSeek Harness.

use portable_pty::PtySize;
use rquickjs::{
    CatchResultExt, Context, FromJs, Function, Module, Object, Runtime, function::IntoArgs,
};
use tauri::AppHandle;

pub(super) struct ScreenStore {
    context: Context,
}

impl ScreenStore {
    pub(super) fn load(app: &AppHandle) -> Result<Self, String> {
        let assets = app.asset_resolver();
        let headless = assets
            .get("xterm-headless.mjs".into())
            .ok_or("找不到终端解析器")?;
        let serializer = assets
            .get("xterm-addon-serialize.mjs".into())
            .ok_or("找不到终端快照组件")?;
        Self::new(headless.bytes(), serializer.bytes())
    }

    fn new(headless: &[u8], serializer: &[u8]) -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|error| error.to_string())?;
        let context = Context::full(&runtime).map_err(|error| error.to_string())?;
        context.with(|ctx| {
            let initialize = || {
                ctx.eval::<(), _>(include_str!("screen.js"))?;
                for (name, source, export) in [
                    ("xterm-headless", headless, "Terminal"),
                    ("xterm-addon-serialize", serializer, "SerializeAddon"),
                ] {
                    let (module, ready) = Module::declare(ctx.clone(), name, source)?.eval()?;
                    ready.finish::<()>()?;
                    ctx.globals()
                        .set(export, module.get::<_, Function>(export)?)?;
                }
                Ok(())
            };
            initialize().catch(&ctx).map_err(|error| error.to_string())
        })?;
        Ok(Self { context })
    }

    fn call<A, R>(&self, name: &str, args: A) -> Result<R, String>
    where
        for<'js> A: IntoArgs<'js>,
        for<'js> R: FromJs<'js>,
    {
        self.context.with(|ctx| {
            let invoke = || {
                let screens: Object = ctx.globals().get("terminalScreens")?;
                screens.get::<_, Function>(name)?.call(args)
            };
            let result = invoke().catch(&ctx).map_err(|error| error.to_string());
            while ctx.execute_pending_job() {}
            result
        })
    }

    pub(super) fn open(&self, id: &str, size: PtySize) -> Result<(), String> {
        self.call("open", (id, size.cols, size.rows))
    }

    pub(super) fn write(&self, id: &str, data: &str) -> Result<(), String> {
        self.call("write", (id, data))
    }

    pub(super) fn resize(&self, id: &str, size: PtySize) -> Result<(), String> {
        self.call("resize", (id, size.cols, size.rows))
    }

    pub(super) fn snapshot(&self, id: &str) -> Result<String, String> {
        self.call("snapshot", (id,))
    }

    pub(super) fn close(&self, id: &str) -> Result<(), String> {
        self.call("close", (id,))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_retain_modes_across_long_output_and_resize() {
        let dependencies =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/@xterm");
        let headless =
            std::fs::read(dependencies.join("headless/lib-headless/xterm-headless.mjs")).unwrap();
        let serializer =
            std::fs::read(dependencies.join("addon-serialize/lib/addon-serialize.mjs")).unwrap();
        let screens = ScreenStore::new(&headless, &serializer).unwrap();
        screens
            .open("first", crate::terminal::terminal_size(80, 24))
            .unwrap();
        screens
            .open("second", crate::terminal::terminal_size(80, 24))
            .unwrap();
        let blank = screens.snapshot("second").unwrap();
        screens.write("first", "\x1b[?2004h").unwrap();
        let chunk = "discarded\r\n".repeat(350);
        for _ in 0..600 {
            screens.write("first", &chunk).unwrap();
        }
        screens
            .resize("first", crate::terminal::terminal_size(35, 10))
            .unwrap();
        screens
            .write("first", "kept prompt\x1b[?1049h\x1b[31m世界")
            .unwrap();
        let snapshot = screens.snapshot("first").unwrap();
        assert!(snapshot.contains("kept prompt"));
        assert!(snapshot.contains("\x1b[?1049h"));
        assert!(snapshot.contains("\x1b[31m世界"));
        assert!(snapshot.ends_with("\x1b[?2004h"));
        assert_eq!(snapshot.matches("discarded").count(), 1009);
        assert_eq!(screens.snapshot("second").unwrap(), blank);
        screens.close("first").unwrap();
        assert!(screens.snapshot("first").is_err());
        assert_eq!(screens.snapshot("second").unwrap(), blank);
    }
}
