pub enum PathKind {
    Prefix,
    Cellar,
}

pub fn execute(
    prefix: &std::path::Path,
    formulas: Vec<String>,
    kind: PathKind,
) -> Result<(), zb_core::Error> {
    if formulas.is_empty() {
        let path = match kind {
            PathKind::Prefix => prefix.to_path_buf(),
            PathKind::Cellar => prefix.join("Cellar"),
        };
        println!("{}", path.display());
        return Ok(());
    }

    for formula in formulas {
        let path = match kind {
            PathKind::Prefix => prefix.join("opt").join(formula),
            PathKind::Cellar => prefix.join("Cellar").join(formula),
        };
        println!("{}", path.display());
    }
    Ok(())
}
