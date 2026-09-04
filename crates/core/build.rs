use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

fn main() -> io::Result<()> {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set by Cargo"),
    );
    let repository_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| io::Error::other("could not locate repository root"))?;
    let facets_root = repository_root.join("builtins/prompts/facets");

    let instructions = read_facet_directory(&facets_root.join("instructions"), &["observer.md"])?;
    let observer_instructions = read_facet_file(&facets_root.join("instructions/observer.md"))?;
    let observer_output_contracts =
        read_facet_directory(&facets_root.join("output-contracts"), &[])?;
    let knowledge = read_facet_directory(&facets_root.join("knowledge"), &[])?;
    let policy = read_facet_directory(&facets_root.join("policies"), &[])?;
    let output = format!(
        "pub const BUILTIN_INSTRUCTIONS: &str = {instructions:?};\n\
pub const BUILTIN_OBSERVER_INSTRUCTIONS: &str = {observer_instructions:?};\n\
pub const BUILTIN_OBSERVER_OUTPUT_CONTRACTS: &str = {observer_output_contracts:?};\n\
pub const BUILTIN_KNOWLEDGE: &str = {knowledge:?};\n\
pub const BUILTIN_POLICY: &str = {policy:?};\n"
    );

    let output_path = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR must be set by Cargo"))
        .join("prompt_facets.rs");
    fs::write(output_path, output)
}

fn read_facet_directory(directory: &Path, excluded_files: &[&str]) -> io::Result<String> {
    println!("cargo:rerun-if-changed={}", directory.display());

    let mut paths = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    paths.retain(|path| {
        path.is_file()
            && path.extension().is_some_and(|extension| extension == "md")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| excluded_files.contains(&name))
    });
    paths.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "facet directory has no markdown files: {}",
                directory.display()
            ),
        ));
    }

    let mut contents = String::new();
    for path in paths {
        println!("cargo:rerun-if-changed={}", path.display());
        contents.push_str(&fs::read_to_string(path)?);
    }
    Ok(contents)
}

fn read_facet_file(path: &Path) -> io::Result<String> {
    println!("cargo:rerun-if-changed={}", path.display());
    fs::read_to_string(path)
}
