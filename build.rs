use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let embedded = env::var_os("CARGO_FEATURE_BUNDLED_RUNTIME").is_some();
    if embedded {
        bundle_runtime(&root);
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/cassette.ico");
        let mut resource = winresource::WindowsResource::new();
        // GPUI supplies the process manifest; add only Cassette's icon and version metadata.
        resource
            .set_icon("assets/cassette.ico")
            .set("ProductName", "Cassette")
            .set(
                "FileDescription",
                if embedded {
                    "Cassette music player with bundled playback"
                } else {
                    "Cassette music player"
                },
            );
        resource
            .compile()
            .expect("Windows resource compilation failed");
    }
    let mut generated =
        String::from("pub fn bundled(path: &str) -> Option<&'static [u8]> { match path {\n");
    println!("cargo:rerun-if-changed=assets/cassette-44.png");
    generated.push_str(&format!(
        "\"raag/app-icon.png\" => Some(include_bytes!({:?})),\n",
        root.join("assets/cassette-44.png").to_string_lossy()
    ));
    println!("cargo:rerun-if-changed=assets/cassette-home.png");
    generated.push_str(&format!(
        "\"raag/cassette-home.png\" => Some(include_bytes!({:?})),\n",
        root.join("assets/cassette-home.png").to_string_lossy()
    ));
    for folder in ["icons", "covers", "fonts"] {
        let directory = root.join("assets").join(folder);
        println!("cargo:rerun-if-changed={}", directory.display());
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|ext| ext == "svg" || ext == "png" || ext == "ttf" || ext == "txt")
                {
                    let name = path.file_name().unwrap().to_string_lossy();
                    generated.push_str(&format!(
                        "\"raag/{folder}/{name}\" => Some(include_bytes!({:?})),\n",
                        path.to_string_lossy()
                    ));
                }
            }
        }
    }
    generated.push_str("_ => None } }\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        generated,
    )
    .unwrap();
}

fn bundle_runtime(root: &Path) {
    let runtime = root.join(".runtime/extractors");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut files = Vec::new();
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    for name in ["yt-dlp", "deno"] {
        let filename = if windows {
            format!("{name}.exe")
        } else {
            name.into()
        };
        assert!(
            runtime.join(filename).is_file(),
            "Missing {name}; run python scripts/bundle-runtime.py before building bundled-runtime"
        );
    }
    assert!(
        runtime.join("versions.json").is_file(),
        "Missing playback tool provenance"
    );
    collect_runtime(&runtime, &runtime, &mut files);
    files.push((root.join("LICENSE"), "licenses/Cassette-LICENSE".into()));
    files.retain(|(_, name)| {
        !name.starts_with("installer/") && !name.ends_with(".bat") && !name.ends_with(".ps1")
    });
    files.sort_by(|left, right| left.1.cmp(&right.1));
    files.dedup_by(|left, right| left.1 == right.1);
    let payload = output.join("runtime.gz");
    let mut compressed = flate2::write::GzEncoder::new(
        fs::File::create(&payload).unwrap(),
        flate2::Compression::default(),
    );
    let mut generated = String::from("pub const FILES: &[(&str, u64, [u8; 32])] = &[\n");
    let mut buffer = [0u8; 64 * 1024];
    for (path, name) in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let mut file = fs::File::open(&path).unwrap();
        let mut hash = Sha256::new();
        let mut size = 0u64;
        loop {
            let read = file.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
            compressed.write_all(&buffer[..read]).unwrap();
            size += read as u64;
        }
        generated.push_str(&format!(
            "({name:?}, {size}, {:?}),\n",
            <[u8; 32]>::from(hash.finalize())
        ));
    }
    compressed.finish().unwrap();
    generated.push_str("];\n");
    let mut hash = Sha256::new();
    let mut file = fs::File::open(&payload).unwrap();
    loop {
        let read = file.read(&mut buffer).unwrap();
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    generated.push_str(&format!(
        "pub const VERSION: &str = \"{:x}\";\npub const PAYLOAD: &[u8] = include_bytes!({:?});\n",
        hash.finalize(),
        payload.to_string_lossy()
    ));
    fs::write(output.join("runtime.rs"), generated).unwrap();
    println!("cargo:rerun-if-changed={}", runtime.display());
}

fn collect_runtime(directory: &Path, root: &Path, files: &mut Vec<(PathBuf, String)>) {
    if !directory.is_dir() {
        return;
    }
    for entry in fs::read_dir(directory).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_runtime(&path, root, files);
        } else if path.is_file() {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.push((path, name));
        }
    }
}
