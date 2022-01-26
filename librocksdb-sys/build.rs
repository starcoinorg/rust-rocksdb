use std::fs;
use std::path::PathBuf;
use std::{env, str};

#[allow(dead_code)]
fn link(name: &str, bundled: bool) {
    use std::env::var;
    let target = var("TARGET").unwrap();
    let target: Vec<_> = target.split('-').collect();
    if target.get(2) == Some(&"windows") {
        println!("cargo:rustc-link-lib=dylib={}", name);
        if bundled && target.get(3) == Some(&"gnu") {
            let dir = var("CARGO_MANIFEST_DIR").unwrap();
            println!("cargo:rustc-link-search=native={}/{}", dir, target[0]);
        }
    }
}

#[allow(dead_code)]
fn fail_on_empty_directory(name: &str) {
    if fs::read_dir(name).unwrap().count() == 0 {
        println!(
            "The `{}` directory is empty, did you forget to pull the submodules?",
            name
        );
        println!("Try `git submodule update --init --recursive`");
        panic!();
    }
}

fn rocksdb_include_dir() -> String {
    match env::var("ROCKSDB_INCLUDE_DIR") {
        Ok(val) => val,
        Err(_) => "rocksdb/include".to_string(),
    }
}

fn bindgen_rocksdb() {
    let bindings = bindgen::Builder::default()
        .header(rocksdb_include_dir() + "/rocksdb/c.h")
        .derive_debug(false)
        .blocklist_type("max_align_t") // https://github.com/rust-lang-nursery/rust-bindgen/issues/550
        .ctypes_prefix("libc")
        .size_t_is_usize(true)
        .generate()
        .expect("unable to generate rocksdb bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("unable to write rocksdb bindings");
}

#[allow(dead_code)]
fn try_to_find_and_link_lib(lib_name: &str) -> bool {
    if let Ok(v) = env::var(&format!("{}_COMPILE", lib_name)) {
        if v.to_lowercase() == "true" || v == "1" {
            return false;
        }
    }

    if let Ok(lib_dir) = env::var(&format!("{}_LIB_DIR", lib_name)) {
        println!("cargo:rustc-link-search=native={}", lib_dir);
        let mode = match env::var_os(&format!("{}_STATIC", lib_name)) {
            Some(_) => "static",
            None => "dylib",
        };
        println!("cargo:rustc-link-lib={}={}", mode, lib_name.to_lowercase());
        return true;
    }
    false
}
#[allow(dead_code)]
fn cxx_standard() -> String {
    env::var("ROCKSDB_CXX_STD").map_or("-std=c++11".to_owned(), |cxx_std| {
        if !cxx_std.starts_with("-std=") {
            format!("-std={}", cxx_std)
        } else {
            cxx_std
        }
    })
}
fn link_cpp(build: &mut cc::Build) {
    let tool = build.get_compiler();
    let stdlib = if tool.is_like_gnu() {
        "libstdc++.a"
    } else if tool.is_like_clang() {
        "libc++.a"
    } else {
        // Don't link to c++ statically on windows.
        return;
    };
    let output = tool
        .to_command()
        .arg("--print-file-name")
        .arg(stdlib)
        .output()
        .unwrap();
    if !output.status.success() || output.stdout.is_empty() {
        // fallback to dynamically
        return;
    }
    let path = match str::from_utf8(&output.stdout) {
        Ok(path) => PathBuf::from(path),
        Err(_) => return,
    };
    if !path.is_absolute() {
        return;
    }
    // remove lib prefix and .a postfix.
    let libname = &stdlib[3..stdlib.len() - 2];
    // static linking
    println!("cargo:rustc-link-lib=static={}", &libname);
    println!(
        "cargo:rustc-link-search=native={}",
        path.parent().unwrap().display()
    );
    build.cpp_link_stdlib(None);
}

fn main() {
    println!("cargo:rerun-if-changed=rocksdb/");
    let mut build = cmake_build_rocksdb();
    bindgen_rocksdb();
    build.cpp(true);
    build.flag("-std=c++11");
    build.flag("-fno-rtti");

    link_cpp(&mut build);
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=c++");
    } else {
        println!("cargo:rustc-link-lib=stdc++");
    }
}

fn cmake_build_rocksdb() -> cc::Build {
    let target = env::var("TARGET").unwrap();

    let mut cmake_cfg = cmake::Config::new("rocksdb");

    if target.contains("x86_64") && cfg!(feature = "sse") {
        // see https://github.com/facebook/rocksdb/blob/v6.20.3/INSTALL.md
        // USE_SSE=1 can't work
        // println!("cargo:rustc-env=USE_SSE=1");
        // see https://github.com/facebook/rocksdb/blob/v6.20.3/CMakeLists.txt#L266
        //cmake_cfg.define("PORTABLE", "ON");
        cmake_cfg.define("FORCE_SSE42", "ON");
    }

    // RocksDB cmake script expect libz.a being under ${DEP_Z_ROOT}/lib, but libz-sys crate put it
    // under ${DEP_Z_ROOT}/build. Append the path to CMAKE_PREFIX_PATH to get around it.
    env::set_var("CMAKE_PREFIX_PATH", {
        let zlib_path = format!("{}", env::var("DEP_Z_ROOT").unwrap());
        if let Ok(prefix_path) = env::var("CMAKE_PREFIX_PATH") {
            format!("{};{}", prefix_path, zlib_path)
        } else {
            zlib_path
        }
    });

    if cfg!(feature = "jemalloc") {
        cmake_cfg
            .register_dep("JEMALLOC")
            .define("WITH_JEMALLOC", "ON");
        println!("cargo:rustc-link-lib=static=jemalloc");
    }

    let dst = cmake_cfg
        .define("WITH_GFLAGS", "OFF")
       // .register_dep("Z")
        //.define("WITH_ZLIB", "ON")
        //.register_dep("BZIP2")
        //.define("WITH_BZ2", "ON")
        .register_dep("LZ4")
        .define("WITH_LZ4", "ON")
        //.register_dep("ZSTD")
        //.define("WITH_ZSTD", "ON")
        .register_dep("SNAPPY")
        .define("WITH_SNAPPY", "ON")
        .define("WITH_TESTS", "OFF")
        .define("WITH_TOOLS", "OFF")
        .build_target("rocksdb")
        .very_verbose(true)
        .build();
    let build_dir = format!("{}/build", dst.display());
    let mut build = cc::Build::new();
    if cfg!(target_os = "windows") {
        let profile = match &*env::var("PROFILE").unwrap_or_else(|_| "debug".to_owned()) {
            "bench" | "release" => "Release",
            _ => "Debug",
        };
        println!("cargo:rustc-link-search=native={}/{}", build_dir, profile);
    } else {
        println!("cargo:rustc-link-search=native={}", build_dir);
    }

    println!("cargo:rustc-link-lib=static=rocksdb");
    //println!("cargo:rustc-link-lib=static=z");
    //println!("cargo:rustc-link-lib=static=bz2");
    println!("cargo:rustc-link-lib=static=lz4");
    //println!("cargo:rustc-link-lib=static=zstd");
    println!("cargo:rustc-link-lib=static=snappy");

    build
}
