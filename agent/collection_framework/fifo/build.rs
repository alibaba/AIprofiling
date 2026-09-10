use std::env;
use std::path::PathBuf;
// use std::fs;

fn convert_header(filename: &str, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    let bindings = bindgen::Builder::default()
        // The input header we would like to generate
        // bindings for.
        .header(format!("cupit_headers/{}", filename))
        // Tell cargo to invalidate the built crate whenever any of the
        // included header files changed.
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: true,
        })
        .raw_line(
            "#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]",
        )
        .impl_partialeq(true)
        .derive_default(true)
        // Finish the builder and generate the bindings.
        .generate()
        // Unwrap the Result and panic on failure.
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join(format!("{}.rs", output)))
        .expect("Couldn't write bindings!");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // for header_file in fs::read_dir("headers")?.filter_map(|f| {
    //     let f = f.ok()?;
    //     f.file_name().to_str().map(|e| e.to_string())
    // }) {
    //     convert_header(&header_file)?
    // }

    #[cfg(all(feature = "protocal_gp", feature = "protocal_ascii"))]
    compile_error!("protocal_gp and protocal_ascii cannot be enabled at the same time");

    #[cfg(not(any(feature = "protocal_gp", feature = "protocal_ascii")))]
    compile_error!("protocal_gp or protocal_ascii must be enabled");

    convert_header("cupti_runtime_cbid.h", "cupti_runtime_cbid")?;

    Ok(())
}
