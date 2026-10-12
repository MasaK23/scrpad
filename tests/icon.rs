//! `main` decodes the embedded app icon at start-up; a broken file must fail here, not there.

#[test]
fn app_icons_decode() {
    for (file, size) in [("scrpad.png", 1024), ("scrpad-256.png", 256)] {
        let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/app-icon").join(file)).unwrap();
        let icon = eframe::icon_data::from_png_bytes(&bytes).unwrap();
        assert_eq!((icon.width, icon.height), (size, size), "{file}");
    }
}
