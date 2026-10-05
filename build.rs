use std::env;

fn main() {
    println!("cargo:rerun-if-changed=assets/DiscordOnlyDPI.ico");
    println!("cargo:rerun-if-changed=tools/rc.exe");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();

        if !env::var("HOST").unwrap_or_default().contains("windows") {
            resource.set_toolkit_path("tools");
            resource.add_toolkit_include(false);
        }

        resource.set_icon("assets/DiscordOnlyDPI.ico");
        resource
            .compile()
            .expect("failed to embed DiscordOnlyDPI icon");
    }
}
