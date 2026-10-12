// Embeds the app icon into scrpad.exe so that Explorer and shortcuts show it.
fn main() {
    println!("cargo::rerun-if-changed=assets/app-icon/scrpad.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app-icon/scrpad.ico");
        res.compile().expect("could not embed the Windows icon");
    }
}
