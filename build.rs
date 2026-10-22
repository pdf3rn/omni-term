fn main() {
    println!("cargo:rerun-if-changed=packaging/omni-term.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("packaging/omni-term.ico");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=no se pudo incrustar el icono: {error}");
        }
    }
}
