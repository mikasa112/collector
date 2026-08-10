//! Build script for the `iec_60870_dev` module.
//!
//! This compiles the vendored `lib60870-C` sources (IEC 60870-5-101/104
//! master/slave library, https://github.com/mz-automation/lib60870) with
//! `cc`, and generates Rust FFI bindings for its public API with `bindgen`.
//!
//! The library currently only builds its Linux HAL backend here (matching
//! the `#[cfg(target_os = "linux")]` gate on the `iec_60870_dev` module in
//! `dev/mod.rs`). TLS support (mbedtls) is not enabled, since the mbedtls
//! sources are not vendored.

use std::env;
use std::path::{Path, PathBuf};

/// Sources shared by all platforms (`lib_common_SRCS` in CMakeLists.txt),
/// relative to `<vendor>/lib60870-C/src`.
const COMMON_SOURCES: &[&str] = &[
    "file-service/file_server.c",
    "iec60870/apl/cpXXtime2a.c",
    "iec60870/cs101/cs101_asdu.c",
    "iec60870/cs101/cs101_bcr.c",
    "iec60870/cs101/cs101_information_objects.c",
    "iec60870/cs101/cs101_master_connection.c",
    "iec60870/cs101/cs101_master.c",
    "iec60870/cs101/cs101_queue.c",
    "iec60870/cs101/cs101_slave.c",
    "iec60870/cs104/cs104_connection.c",
    "iec60870/cs104/cs104_frame.c",
    "iec60870/cs104/cs104_slave.c",
    "iec60870/link_layer/buffer_frame.c",
    "iec60870/link_layer/link_layer.c",
    "iec60870/link_layer/serial_transceiver_ft_1_2.c",
    "iec60870/frame.c",
    "iec60870/lib60870_common.c",
    // BUILD_COMMON
    "common/linked_list.c",
];

/// Linux HAL sources (`lib_linux_SRCS` in CMakeLists.txt).
const LINUX_HAL_SOURCES: &[&str] = &[
    "hal/serial/linux/serial_port_linux.c",
    "hal/socket/linux/socket_linux.c",
    "hal/thread/linux/thread_linux.c",
    "hal/time/unix/time.c",
    "hal/memory/lib_memory.c",
];

/// Public headers exposed to Rust via bindgen (`API_HEADERS` in CMakeLists.txt).
const API_HEADERS: &[&str] = &[
    "hal/inc/hal_time.h",
    "hal/inc/hal_thread.h",
    "hal/inc/hal_socket.h",
    "hal/inc/hal_serial.h",
    "hal/inc/hal_base.h",
    "hal/inc/tls_config.h",
    "hal/inc/tls_ciphers.h",
    "common/inc/linked_list.h",
    "inc/api/cs101_master.h",
    "inc/api/cs101_slave.h",
    "inc/api/cs104_slave.h",
    "inc/api/iec60870_master.h",
    "inc/api/iec60870_slave.h",
    "inc/api/iec60870_common.h",
    "inc/api/cs101_information_objects.h",
    "inc/api/cs104_connection.h",
    "inc/api/link_layer_parameters.h",
    "file-service/cs101_file_service.h",
];

fn main() {
    // The lib60870 HAL is only vendored/built for Linux here, matching the
    // `#[cfg(target_os = "linux")]` gate on the `iec_60870_dev` module.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        println!("cargo:warning=iec_60870_dev: skipping lib60870-C build, target_os is not linux");
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_root = manifest_dir.join("vendor/lib60870/lib60870-C");
    let src_dir = vendor_root.join("src");

    if !src_dir.exists() {
        panic!(
            "lib60870-C sources not found at {}. Did you run `git submodule update --init --recursive`?",
            src_dir.display()
        );
    }

    let config_dir = vendor_root.join("config");
    let api_inc_dir = src_dir.join("inc/api");
    let internal_inc_dir = src_dir.join("inc/internal");
    let common_inc_dir = src_dir.join("common/inc");
    let hal_inc_dir = src_dir.join("hal/inc");
    let file_service_dir = src_dir.join("file-service");

    let include_dirs = [
        &config_dir,
        &api_inc_dir,
        &internal_inc_dir,
        &common_inc_dir,
        &hal_inc_dir,
        &file_service_dir,
    ];

    compile_library(&src_dir, &include_dirs);
    generate_bindings(&vendor_root, &include_dirs);

    // Rebuild if the vendored library or this build script changes.
    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-changed={}", config_dir.display());
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
}

fn compile_library(src_dir: &Path, include_dirs: &[&PathBuf]) {
    let mut build = cc::Build::new();

    build
        .warnings(false)
        .flag_if_supported("-Wno-unused-parameter");

    for dir in include_dirs {
        build.include(dir);
    }

    for rel in COMMON_SOURCES.iter().chain(LINUX_HAL_SOURCES.iter()) {
        let path = src_dir.join(rel);
        if !path.exists() {
            panic!(
                "expected lib60870-C source file not found: {}",
                path.display()
            );
        }
        build.file(path);
    }

    build.compile("lib60870");

    // lib60870-C relies on pthreads, libm and librt on Linux.
    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=rt");
}

fn generate_bindings(vendor_root: &Path, include_dirs: &[&PathBuf]) {
    let src_dir = vendor_root.join("src");

    let mut wrapper = String::new();
    for rel in API_HEADERS {
        wrapper.push_str(&format!("#include \"{}\"\n", src_dir.join(rel).display()));
    }

    let mut builder = bindgen::Builder::default()
        .header_contents("iec_60870_dev_wrapper.h", &wrapper)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_default(true)
        .generate_comments(true)
        .allowlist_file(format!("{}/.*", src_dir.display()))
        .clang_arg("-D_GNU_SOURCE");

    for dir in include_dirs {
        builder = builder.clang_arg(format!("-I{}", dir.display()));
    }

    let bindings = builder
        .generate()
        .expect("failed to generate lib60870 bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write lib60870 bindings");
}
