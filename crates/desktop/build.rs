//! Embeds the icon and the manifest into the Windows executable.

fn main() {
    // Another host's `cargo check --target` must not look for a resource compiler.
    if std::env::var_os("CARGO_CFG_TARGET_OS").is_none_or(|os| os != "windows") {
        return;
    }
    let mut resources = winresource::WindowsResource::new();
    resources
        .set_icon("assets/app.ico")
        .set_manifest_file("assets/app.manifest");
    if let Err(e) = resources.compile() {
        panic!("could not embed the icon and manifest: {e}");
    }
}
