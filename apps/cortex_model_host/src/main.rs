use cortex_model_host::{discover_models, run_supervisor, LLAMA_CPP_RELEASE};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(error) = run() {
        eprintln!("Cortex model host failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str).unwrap_or("status") {
        "run" => {
            let library = value(&args, "--library-root")
                .map(PathBuf::from)
                .ok_or_else(|| "--library-root is required".to_string())?;
            let parent_pid = value(&args, "--parent-pid")
                .ok_or_else(|| "--parent-pid is required".to_string())?
                .parse::<u32>()
                .map_err(|error| error.to_string())?;
            let port = value(&args, "--port")
                .unwrap_or_else(|| "12400".into())
                .parse::<u16>()
                .map_err(|error| error.to_string())?;
            let models_max = value(&args, "--models-max")
                .unwrap_or_else(|| "2".into())
                .parse::<u8>()
                .map_err(|error| error.to_string())?
                .clamp(1, 8);
            let auto_bootstrap = !args.iter().any(|arg| arg == "--no-auto-bootstrap");
            run_supervisor(library, parent_pid, port, auto_bootstrap, models_max)
        }
        "scan" => {
            let root = value(&args, "--models-dir")
                .map(PathBuf::from)
                .unwrap_or_else(configured_models_root);
            let catalog = discover_models(root)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&catalog).map_err(|error| error.to_string())?
            );
            Ok(())
        }
        "version" => {
            println!("Cortex model host / llama.cpp {LLAMA_CPP_RELEASE}");
            Ok(())
        }
        other => Err(format!("unknown cortex_model_host command: {other}")),
    }
}

fn value(args: &[String], key: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == key)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn configured_models_root() -> PathBuf {
    if let Some(root) = std::env::var_os("CORTEX_MODELS_ROOT").map(PathBuf::from) {
        return root;
    }

    if let Some(root) = std::env::var_os("CORTEX_LIBRARY_ROOT").map(PathBuf::from) {
        return models_root_for(&root);
    }

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        let record_path = local_app_data
            .join("Cortex")
            .join("registry")
            .join("library.json");
        if let Some(root) = std::fs::read(&record_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|value| {
                value
                    .get("root")
                    .and_then(serde_json::Value::as_str)
                    .map(PathBuf::from)
            })
        {
            return models_root_for(&root);
        }
    }

    #[cfg(windows)]
    {
        if Path::new(r"D:\").exists() {
            return PathBuf::from(r"D:\Cortex\Models");
        }
    }

    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Cortex")
        .join("Models")
}

fn models_root_for(storage_root: &Path) -> PathBuf {
    #[cfg(windows)]
    if storage_root.components().count() <= 2 {
        return storage_root.join("Cortex").join("Models");
    }

    storage_root.join("Models")
}
