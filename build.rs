use std::env;

fn main() {
    println!("cargo:rerun-if-changed=assets/DiscordOnlyDPI.ico");
    println!("cargo:rerun-if-changed=tools/rc.exe");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string());
        let mut resource = winres::WindowsResource::new();

        if !env::var("HOST").unwrap_or_default().contains("windows") {
            resource.set_toolkit_path("tools");
            resource.add_toolkit_include(false);
        }

        resource
            .set_icon("assets/DiscordOnlyDPI.ico")
            .set("ProductName", "DiscordOnlyDPI")
            .set(
                "FileDescription",
                "Discord-only DPI bypass utility for Windows",
            )
            .set("ProductVersion", &version)
            .set("FileVersion", &version)
            .set("OriginalFilename", "DiscordOnlyDPI.exe")
            .set("InternalName", "DiscordOnlyDPI")
            .set(
                "Comments",
                "Discord-only local proxy utility powered by ByeDPI",
            );

        resource
            .compile()
            .expect("failed to compile DiscordOnlyDPI Windows resources");
    }
}
