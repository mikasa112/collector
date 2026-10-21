//! Build script for `collector-iec61850`.
//!
//! 用 `cc` 编译 vendored 的 libiec61850 (https://github.com/mz-automation/libiec61850,
//! IEC 61850 MMS 客户端/服务端), 并用 `bindgen` 生成 FFI 绑定。
//!
//! 与 `collector-iec60870` 一致, 目前只构建 Linux HAL。不编译 GOOSE / SV / 以太网 HAL,
//! 也不启用 TLS(mbedtls), 见 `config/stack_config.h`。
//!
//! 服务端源码**始终编译**: 客户端用到的 `mmsMsg_createMmsRejectPdu` 等函数定义在服务端文件里
//! (上游本来就把二者编进同一个库), 只编客户端会在最终链接时缺符号。未用到的服务端代码由
//! 链接器的 `--gc-sections` 丢弃。`server` feature 只决定是否生成并公开服务端的 FFI 绑定,
//! 供测试用 IED 使用。

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 客户端及公共部分, 相对 `<vendor>/src`。
const CLIENT_SOURCES: &[&str] = &[
    "common/string_map.c",
    "common/map.c",
    "common/linked_list.c",
    "common/byte_buffer.c",
    "common/string_utilities.c",
    "common/buffer_chain.c",
    "common/conversions.c",
    "common/mem_alloc_linked_list.c",
    "common/simple_allocator.c",
    "mms/iso_acse/acse.c",
    "mms/iso_mms/common/mms_type_spec.c",
    "mms/iso_mms/common/mms_value.c",
    "mms/iso_mms/common/mms_common_msg.c",
    "mms/iso_mms/client/mms_client_initiate.c",
    "mms/iso_mms/client/mms_client_write.c",
    "mms/iso_mms/client/mms_client_identify.c",
    "mms/iso_mms/client/mms_client_status.c",
    "mms/iso_mms/client/mms_client_named_variable_list.c",
    "mms/iso_mms/client/mms_client_connection.c",
    "mms/iso_mms/client/mms_client_files.c",
    "mms/iso_mms/client/mms_client_get_namelist.c",
    "mms/iso_mms/client/mms_client_get_var_access.c",
    "mms/iso_mms/client/mms_client_common.c",
    "mms/iso_mms/client/mms_client_read.c",
    "mms/iso_mms/client/mms_client_journals.c",
    "mms/iso_cotp/cotp.c",
    "mms/iso_presentation/iso_presentation.c",
    "mms/asn1/ber_decode.c",
    "mms/asn1/asn1_ber_primitive_value.c",
    "mms/asn1/ber_encoder.c",
    "mms/asn1/ber_integer.c",
    "mms/iso_client/iso_client_connection.c",
    "mms/iso_common/iso_connection_parameters.c",
    "mms/iso_session/iso_session.c",
    "iec61850/client/client_control.c",
    "iec61850/client/client_report_control.c",
    "iec61850/client/client_goose_control.c",
    "iec61850/client/client_sv_control.c",
    "iec61850/client/client_report.c",
    "iec61850/client/ied_connection.c",
    "iec61850/common/iec61850_common.c",
    "logging/log_storage.c",
];

/// 服务端部分, 相对 `<vendor>/src`。
const SERVER_SOURCES: &[&str] = &[
    "mms/iso_server/iso_connection.c",
    "mms/iso_server/iso_server.c",
    "mms/iso_mms/server/mms_read_service.c",
    "mms/iso_mms/server/mms_file_service.c",
    "mms/iso_mms/server/mms_association_service.c",
    "mms/iso_mms/server/mms_identify_service.c",
    "mms/iso_mms/server/mms_status_service.c",
    "mms/iso_mms/server/mms_named_variable_list_service.c",
    "mms/iso_mms/server/mms_value_cache.c",
    "mms/iso_mms/server/mms_get_namelist_service.c",
    "mms/iso_mms/server/mms_access_result.c",
    "mms/iso_mms/server/mms_server.c",
    "mms/iso_mms/server/mms_server_common.c",
    "mms/iso_mms/server/mms_named_variable_list.c",
    "mms/iso_mms/server/mms_domain.c",
    "mms/iso_mms/server/mms_device.c",
    "mms/iso_mms/server/mms_information_report.c",
    "mms/iso_mms/server/mms_journal.c",
    "mms/iso_mms/server/mms_journal_service.c",
    "mms/iso_mms/server/mms_server_connection.c",
    "mms/iso_mms/server/mms_write_service.c",
    "mms/iso_mms/server/mms_get_var_access_service.c",
    "iec61850/server/impl/ied_server.c",
    "iec61850/server/impl/ied_server_config.c",
    "iec61850/server/impl/client_connection.c",
    "iec61850/server/model/model.c",
    "iec61850/server/model/dynamic_model.c",
    "iec61850/server/model/cdc.c",
    "iec61850/server/model/config_file_parser.c",
    "iec61850/server/mms_mapping/control.c",
    "iec61850/server/mms_mapping/mms_mapping.c",
    "iec61850/server/mms_mapping/reporting.c",
    "iec61850/server/mms_mapping/mms_goose.c",
    "iec61850/server/mms_mapping/mms_sv.c",
    "iec61850/server/mms_mapping/logging.c",
];

/// Linux HAL, 相对 `<vendor>/hal`。
const LINUX_HAL_SOURCES: &[&str] = &[
    "socket/linux/socket_linux.c",
    "thread/linux/thread_linux.c",
    "filesystem/linux/file_provider_linux.c",
    "time/unix/time.c",
    "memory/lib_memory.c",
];

/// 暴露给 Rust 的公共头文件, 相对 `<vendor>`。
const API_HEADERS: &[&str] = &[
    "hal/inc/hal_base.h",
    "hal/inc/hal_time.h",
    "hal/inc/hal_thread.h",
    "src/common/inc/linked_list.h",
    "src/mms/inc/mms_value.h",
    "src/mms/inc/mms_types.h",
    "src/mms/inc/mms_type_spec.h",
    "src/mms/inc/mms_common.h",
    "src/mms/inc/iso_connection_parameters.h",
    "src/iec61850/inc/iec61850_common.h",
    "src/iec61850/inc/iec61850_client.h",
];

/// 服务端头文件(仅 `server` feature)。
const SERVER_HEADERS: &[&str] = &[
    "src/iec61850/inc/iec61850_model.h",
    "src/iec61850/inc/iec61850_cdc.h",
    "src/iec61850/inc/iec61850_dynamic_model.h",
    "src/iec61850/inc/iec61850_server.h",
];

/// libiec61850 和 lib60870 同出一个作者, 都自带 HAL(socket/thread/时间/信号量)、`linked_list`
/// 和 `lib_memory`, 导出的 C 符号同名。两个库链接进同一个可执行文件时:
///
/// - HAL 目前逐字节一致, 碰巧无害;
/// - `linked_list.c` 两边不同(libiec61850 对 NULL 做了防护, lib60870 没有), 链接时谁的版本
///   "赢"取决于链接顺序, libiec61850 的出错路径可能因此落到不带防护的版本上崩溃;
/// - 任何一边升级子模块后, 这种"碰巧一致"都会悄悄失效。
///
/// 所以编译 libiec61850 时给这些符号统一加前缀, 让每个库使用自己私有的一份。符号列表不手写,
/// 而是从下面这些头文件里按前缀扫描, 升级子模块后依然有效。
const SHARED_SYMBOL_HEADERS: &[&str] = &[
    "hal/inc/hal_socket.h",
    "hal/inc/hal_thread.h",
    "hal/inc/hal_time.h",
    "hal/inc/lib_memory.h",
    "src/common/inc/linked_list.h",
];
const SHARED_SYMBOL_PREFIXES: &[&str] = &[
    "Hal_",
    "Handleset_",
    "LinkedList_",
    "Memory_",
    "Semaphore_",
    "ServerSocket_",
    "Socket_",
    "TcpServerSocket_",
    "TcpSocket_",
    "Thread_",
    "UdpSocket_",
];
/// 改名后的符号在 Rust 绑定里同样带这个前缀, 如 `iec61850_LinkedList_getNext`。
const SYMBOL_RENAME_PREFIX: &str = "iec61850_";

/// 测试用 IED 的模型来源: 真实 PCS 的 ICD, 由 libiec61850 自带的 `genmodel.jar` 转成静态 C 模型。
const PCS_ICD: &str = "tests/data/INPOWER_MMS_1.5.icd";
const PCS_IED_NAME: &str = "PCS0001";
const PCS_AP_NAME: &str = "S1";
/// 生成代码里的全局变量前缀, 即 `extern IedModel pcs;`。
const PCS_MODEL_PREFIX: &str = "pcs";

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        println!(
            "cargo:warning=collector-iec61850: skipping libiec61850 build, target_os is not linux"
        );
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest_dir.join("vendor/libiec61850");
    let src_dir = vendor.join("src");
    let hal_dir = vendor.join("hal");
    if !src_dir.exists() {
        panic!(
            "libiec61850 sources not found at {}. Did you run `git submodule update --init --recursive`?",
            src_dir.display()
        );
    }
    let server = env::var_os("CARGO_FEATURE_SERVER").is_some();

    // 自带的 config 必须排在最前, 覆盖 vendor/config/stack_config.h。
    let include_dirs: Vec<PathBuf> = vec![
        manifest_dir.join("config"),
        src_dir.join("common/inc"),
        src_dir.join("mms/inc"),
        src_dir.join("mms/inc_private"),
        src_dir.join("mms/iso_mms/asn1c"),
        src_dir.join("iec61850/inc"),
        src_dir.join("iec61850/inc_private"),
        src_dir.join("logging"),
        hal_dir.join("inc"),
    ];

    // 仅测试需要: 用 ICD 生成静态模型。放在 OUT_DIR, 不进仓库。
    let pcs_model = server.then(|| generate_pcs_model(&manifest_dir, &vendor));

    let renames = shared_symbol_renames(&vendor);
    compile_library(
        &src_dir,
        &hal_dir,
        &include_dirs,
        pcs_model.as_deref(),
        &renames,
    );
    generate_bindings(
        &vendor,
        &include_dirs,
        server,
        pcs_model.is_some(),
        &renames,
    );

    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-changed={}", hal_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("config").display()
    );
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
}

/// 扫描头文件里以 `SHARED_SYMBOL_PREFIXES` 开头、后面跟着 `(` 的标识符, 即被声明的函数名。
fn shared_symbol_renames(vendor: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for header in SHARED_SYMBOL_HEADERS {
        let path = vendor.join(header);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let bytes = text.as_bytes();
        let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut i = 0;
        while i < bytes.len() {
            if !is_ident(bytes[i]) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && is_ident(bytes[i]) {
                i += 1;
            }
            let ident = &text[start..i];
            let next = text[i..].trim_start().chars().next();
            if next == Some('(') && SHARED_SYMBOL_PREFIXES.iter().any(|p| ident.starts_with(p)) {
                names.insert(ident.to_string());
            }
        }
    }
    assert!(
        names.len() >= 40,
        "只扫描到 {} 个共享符号, 头文件格式可能变了: {names:?}",
        names.len()
    );
    names
}

/// 调 `genmodel.jar` 生成 `<OUT_DIR>/pcs_model.{c,h}`, 返回 `.c` 的路径。需要本机有 `java`。
fn generate_pcs_model(manifest_dir: &Path, vendor: &Path) -> PathBuf {
    let icd = manifest_dir.join(PCS_ICD);
    let jar = vendor.join("tools/model_generator/genmodel.jar");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    for f in [&icd, &jar] {
        if !f.exists() {
            panic!("缺少文件 {}", f.display());
        }
    }
    let output = Command::new("java")
        .current_dir(&out_dir)
        .arg("-jar")
        .arg(&jar)
        .arg(&icd)
        .args(["-ied", PCS_IED_NAME, "-ap", PCS_AP_NAME, "-out", "pcs_model", "-modelprefix", PCS_MODEL_PREFIX])
        .output()
        .unwrap_or_else(|e| {
            panic!("启用 `server` feature 需要本机安装 java 来运行 genmodel.jar(用 ICD 生成测试模型): {e}")
        });
    let c_file = out_dir.join("pcs_model.c");
    if !output.status.success() || !c_file.exists() {
        panic!(
            "genmodel 失败 ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    println!("cargo:rerun-if-changed={}", icd.display());
    c_file
}

fn compile_library(
    src_dir: &Path,
    hal_dir: &Path,
    include_dirs: &[PathBuf],
    pcs_model: Option<&Path>,
    renames: &BTreeSet<String>,
) {
    let mut build = cc::Build::new();
    build
        .warnings(false)
        .flag_if_supported("-Wno-unused-parameter")
        .define("_GNU_SOURCE", None);
    for name in renames {
        build.define(name, Some(format!("{SYMBOL_RENAME_PREFIX}{name}").as_str()));
    }
    for dir in include_dirs {
        build.include(dir);
    }

    let mut files: Vec<PathBuf> = CLIENT_SOURCES
        .iter()
        .chain(SERVER_SOURCES.iter())
        .map(|r| src_dir.join(r))
        .collect();
    files.extend(LINUX_HAL_SOURCES.iter().map(|r| hal_dir.join(r)));

    files.extend(pcs_model.map(Path::to_path_buf));

    // asn1c 生成的 MMS 编解码, 文件很多, 直接遍历目录。
    let asn1c = src_dir.join("mms/iso_mms/asn1c");
    let entries =
        fs::read_dir(&asn1c).unwrap_or_else(|e| panic!("cannot read {}: {e}", asn1c.display()));
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "c") {
            files.push(path);
        }
    }

    for path in &files {
        if !path.exists() {
            panic!(
                "expected libiec61850 source file not found: {}",
                path.display()
            );
        }
        build.file(path);
    }
    build.compile("iec61850");

    println!("cargo:rustc-link-lib=pthread");
    println!("cargo:rustc-link-lib=m");
    println!("cargo:rustc-link-lib=rt");
}

fn generate_bindings(
    vendor: &Path,
    include_dirs: &[PathBuf],
    server: bool,
    pcs_model: bool,
    renames: &BTreeSet<String>,
) {
    let mut wrapper = String::new();
    let headers = API_HEADERS
        .iter()
        .chain(if server { SERVER_HEADERS } else { &[] }.iter());
    for rel in headers {
        wrapper.push_str(&format!("#include \"{}\"\n", vendor.join(rel).display()));
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    if pcs_model {
        wrapper.push_str(&format!(
            "#include \"{}\"\n",
            out_dir.join("pcs_model.h").display()
        ));
    }

    let mut builder = bindgen::Builder::default()
        .header_contents("iec61850_wrapper.h", &wrapper)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .derive_default(true)
        .generate_comments(true)
        .allowlist_file(format!("{}/(src|hal)/.*", vendor.display()))
        .clang_arg("-D_GNU_SOURCE");
    for name in renames {
        builder = builder.clang_arg(format!("-D{name}={SYMBOL_RENAME_PREFIX}{name}"));
    }
    if pcs_model {
        // 生成的头文件有几千个 extern 变量, 只导出模型根 `pcs`, 其余通过对象引用在运行时查找。
        builder = builder.allowlist_var(format!("^{PCS_MODEL_PREFIX}$"));
    }
    for dir in include_dirs {
        builder = builder.clang_arg(format!("-I{}", dir.display()));
    }

    let bindings = builder
        .generate()
        .expect("failed to generate libiec61850 bindings");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("bindings.rs");
    bindings
        .write_to_file(&out_path)
        .expect("failed to write libiec61850 bindings");
}
