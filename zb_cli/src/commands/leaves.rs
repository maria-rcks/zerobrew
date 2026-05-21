pub fn execute(installer: &mut zb_io::Installer) -> Result<(), zb_core::Error> {
    for keg in installer.list_installed()? {
        if !keg.name.starts_with("cask:") {
            println!("{}", keg.name);
        }
    }

    Ok(())
}
