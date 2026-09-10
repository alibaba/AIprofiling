// build.rs
use bindgen;
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn build_injector() {
    let bindings = bindgen::Builder::default()
        .header("src/third_party/profiler/include/profiler.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate bindings");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("injector.rs"))
        .expect("Couldn't write bindings!");

    let profiler_lib_path = PathBuf::from("src/third_party/profiler/src/libprofiler.a");
    let target_lib_path = out_path.join("libprofiler.a");

    std::fs::copy(&profiler_lib_path, &target_lib_path)
        .expect("Failed to copy libprofiler.a to OUT_DIR");

    let linker_script_path = PathBuf::from("src/third_party/profiler/link.ld");
    let target_linker_script_path = out_path.join("link.ld");

    std::fs::copy(&linker_script_path, &target_linker_script_path)
        .expect("Failed to copy link.ld to OUT_DIR");

    println!("cargo:rustc-link-search=native={}", out_path.display());
    println!("cargo:rustc-link-lib=static=profiler");
    println!(
        "cargo:rustc-link-arg=-Wl,-T{}",
        target_linker_script_path.display()
    );
    // Hide libprofiler.a symbols from the final binary's dynamic symbol
    // table. Combined with per-object localization done by
    // tools/prepare-libprofiler.sh, this keeps internal function names off
    // both `nm libprofiler.a` and `nm CollectionFramework` / `objdump -T`.
    println!("cargo:rustc-link-arg=-Wl,--exclude-libs=libprofiler.a");
}

fn build_pystack_collector() {
    let pystack_collector_dir = PathBuf::from("src/plugins/pystackCollector/build");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let pystack_lib_path = pystack_collector_dir.join("libpystackcollector.a");
    let target_lib_path = out_dir.join("libpystackcollector.a");
    std::fs::copy(&pystack_lib_path, &target_lib_path)
        .expect("Failed to copy libpystackcollector.a to OUT_DIR");

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=pystackcollector");
}

fn build_pyki_loader() {
    let loader_dir = PathBuf::from("src/plugins/pykiLoader");
    let make_status = Command::new("make")
        .current_dir(&loader_dir)
        .status()
        .expect("Failed to make pyki loader");

    if !make_status.success() {
        panic!("pyki loader compilation failed");
    }

    if let Some(profile_dir) = target_profile_dir() {
        let src = loader_dir.join("libloader.so");
        let dst = profile_dir.join("libloader.so");
        let _ = std::fs::copy(&src, &dst);
    }
}

fn build_pyki_plugin() {
    let pyki_dev_dir = PathBuf::from("src/plugins/pyki/pyki_dev_dir");
    if !pyki_dev_dir.exists() {
        println!(
            "cargo:warning=vendored pyki dir missing at {}; PYKI collector runtime dispatch will fail",
            pyki_dev_dir.display()
        );
        println!("cargo:rerun-if-changed=src/plugins/pyki/pyki_dev_dir");
        return;
    }

    if let Some(profile_dir) = target_profile_dir() {
        let dst = profile_dir.join("pyki_dir");
        let _ = std::fs::remove_dir_all(&dst);
        if let Err(e) = copy_dir_recursive(&pyki_dev_dir, &dst) {
            println!(
                "cargo:warning=failed to stage pyki_dir next to binary: {}",
                e
            );
        }
    }

    println!("cargo:rerun-if-changed=src/plugins/pyki/pyki_dev_dir");
}

fn target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    out_dir.parent()?.parent()?.parent().map(PathBuf::from)
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if ft.is_file() {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn build_cuprof_plugin() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let cuprof_dir = PathBuf::from("src/plugins/cuprof");

    println!("cargo:rerun-if-changed=src/plugins/cuprof/src");
    println!("cargo:rerun-if-changed=src/plugins/cuprof/Makefile");
    println!("cargo:rerun-if-changed=src/plugins/cuprof/export.map");
    println!("cargo:rerun-if-env-changed=CUDA_HOME");

    let cuda_home = env::var("CUDA_HOME").unwrap_or_else(|_| "/usr/local/cuda".to_string());
    let cupti_include = PathBuf::from(&cuda_home).join("extras/CUPTI/include/cupti.h");
    if !cupti_include.exists() {
        // Don't panic when CUPTI is missing, so `cargo check` and dev machines
        // without a CUDA install can still build. If GPU kernel collection is
        // actually needed at runtime, config.yaml will look for libcuprof.so
        // and users will see a clear error when starting CollectionFramework.
        println!(
            "cargo:warning=CUPTI headers not found at {}; skipping cuprof build. \
             Set CUDA_HOME to a CUDA install with extras/CUPTI to enable GPU kernel collection.",
            cupti_include.display()
        );
        return;
    }

    let make_status = Command::new("make")
        .arg(format!("CUDA_HOME={}", cuda_home))
        .arg("build/libcuprof.so")
        .current_dir(&cuprof_dir)
        .status()
        .expect("Failed to run make for cuprof");

    if !make_status.success() {
        panic!("cuprof build failed (make -C {})", cuprof_dir.display());
    }

    let src_lib = cuprof_dir.join("build/libcuprof.so");
    let dst_lib = out_dir.join("libcuprof.so");
    std::fs::copy(&src_lib, &dst_lib).expect("Failed to copy libcuprof.so to OUT_DIR");

    if let Some(profile_dir) = target_profile_dir() {
        let _ = std::fs::copy(&src_lib, profile_dir.join("libcuprof.so"));
    }
}

fn get_git_commit_id() -> String {
    let output = Command::new("git")
        .args(&["rev-parse", "--short=8", "HEAD"])
        .output();

    match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    }
}

fn main() {
    let commit_id = get_git_commit_id();
    println!("cargo:rustc-env=GIT_COMMIT_ID={}", commit_id);
    println!("cargo:rerun-if-changed=../.git/HEAD");

    build_injector();
    build_pystack_collector();
    build_pyki_plugin();
    build_pyki_loader();
    build_cuprof_plugin();
}
