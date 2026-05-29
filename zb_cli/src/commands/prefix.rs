pub fn execute(prefix: &std::path::Path, formulas: Vec<String>) -> Result<(), zb_core::Error> {
    if formulas.is_empty() {
        println!("{}", prefix.display());
        return Ok(());
    }

    for formula in formulas {
        println!("{}", prefix.join("Cellar").join(formula).display());
    }
    Ok(())
}
