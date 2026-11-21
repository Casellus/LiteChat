// Embeds the icon and version info into the .exe (Windows).
fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=assets/litechat.ico");
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/litechat.ico")
                .set("ProductName", "LiteChat")
                .set("FileDescription", "LiteChat")
                .set("InternalName", "litechat.exe")
                .set("OriginalFilename", "litechat.exe");
            res.compile().expect("failed to embed the icon");
        }
    }
}
