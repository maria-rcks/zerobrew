pub fn execute(formulas: Vec<String>) -> Result<(), zb_core::Error> {
    for formula in formulas {
        println!("Warning: pinning is not persisted yet for {formula}");
    }
    Ok(())
}
