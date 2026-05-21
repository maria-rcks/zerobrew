use std::path::Path;

pub fn execute(root: &Path, prefix: &Path) -> Result<(), zb_core::Error> {
    println!("ZEROBREW_ROOT: {}", root.display());
    println!("HOMEBREW_PREFIX: {}", prefix.display());
    println!("HOMEBREW_CELLAR: {}", prefix.join("Cellar").display());
    println!("ZEROBREW_CACHE: {}", root.join("cache").display());
    println!("ZEROBREW_STORE: {}", root.join("store").display());
    Ok(())
}
