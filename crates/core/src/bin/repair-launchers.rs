use appimage_control_core::{repair_instance_launchers, AppPaths, Catalog};

fn main() {
    let paths = AppPaths::from_env().expect("paths");
    let catalog = Catalog::discover_load(None).expect("catalog");

    let mut log = String::new();
    let _ = repair_instance_launchers(&paths, &catalog, &mut |m| {
        log.push_str(m);
        log.push('\n');
    });

    print!("{}", log);
}
