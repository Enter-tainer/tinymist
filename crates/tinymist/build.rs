//! Generates project metadata.

use anyhow::Result;
use vergen::EmitBuilder;

fn main() -> Result<()> {
    // Emit the instructions
    EmitBuilder::builder()
        .all_cargo()
        .build_timestamp()
        .git_sha(false)
        .git_describe(true, true, None)
        .all_rustc()
        .emit()?;

    let metadata = cargo_metadata::MetadataCommand::new().exec().unwrap();
    let typst = metadata
        .packages
        .iter()
        .find(|package| package.name == "typst")
        .expect("Typst should be a dependency");

    println!("cargo:rustc-env=TYPST_VERSION={}", typst.version);

    nlprule_build::BinaryBuilder::new(
        &["en"],
        std::env::var("OUT_DIR").expect("OUT_DIR is set when build.rs is running"),
    )
    .build()?
    .validate()?;

    Ok(())
}
