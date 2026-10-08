fn main() {
    // The Slint UI is compiled only for the `gui` feature, so the light core
    // (what ZenTorrent links) never pays for slint-build.
    #[cfg(feature = "gui")]
    slint_build::compile("ui/main_awesome.slint").unwrap();
}
