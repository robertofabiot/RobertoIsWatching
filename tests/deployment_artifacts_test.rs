use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

struct TempTestDir {
    path: std::path::PathBuf,
}

impl TempTestDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "{}_{}_{}",
            prefix,
            std::process::id(),
            rand::random::<u32>()
        ));
        fs::create_dir_all(&path).expect("failed to create temporary test directory");
        Self { path }
    }
}

impl Drop for TempTestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn test_systemd_service_unit_directives() {
    let content = fs::read_to_string("deploy/robertos-watching.service")
        .expect("deploy/robertos-watching.service must exist");

    assert!(content.contains("Description="));
    assert!(content.contains("User=robertos"));
    assert!(content.contains("Group=robertos"));
    assert!(content.contains("WorkingDirectory=/opt/robertos-watching"));
    assert!(content.contains("ExecStart=/opt/robertos-watching/robertos-watching-bot"));
    assert!(content.contains("EnvironmentFile=/opt/robertos-watching/.env"));
    assert!(content.contains("Restart=always"));
    assert!(content.contains("RestartSec=10"));
    assert!(content.contains("TimeoutStopSec=30"));
    assert!(content.contains("NoNewPrivileges=true"));
    assert!(content.contains("ProtectSystem=strict"));
    assert!(content.contains("ProtectHome=true"));
    assert!(content.contains("PrivateTmp=true"));
    assert!(content.contains("ReadWritePaths=/opt/robertos-watching"));
    assert!(content.contains("WantedBy=multi-user.target"));
}

#[test]
fn test_systemd_backup_units_linkage_and_hardening() {
    let service_content = fs::read_to_string("deploy/robertos-watching-backup.service")
        .expect("deploy/robertos-watching-backup.service must exist");
    assert!(service_content.contains("Type=oneshot"));
    assert!(service_content.contains("User=robertos"));
    assert!(service_content.contains("Group=robertos"));
    assert!(service_content.contains("ExecStart=/opt/robertos-watching/backup.sh"));
    assert!(service_content.contains("EnvironmentFile=-/opt/robertos-watching/.env"));
    assert!(service_content.contains("NoNewPrivileges=true"));
    assert!(service_content.contains("ProtectSystem=strict"));
    assert!(service_content.contains("ReadWritePaths=/opt/robertos-watching"));

    let timer_content = fs::read_to_string("deploy/robertos-watching-backup.timer")
        .expect("deploy/robertos-watching-backup.timer must exist");
    assert!(timer_content.contains("OnCalendar="));
    assert!(timer_content.contains("RandomizedDelaySec="));
    assert!(timer_content.contains("Persistent=true"));
    assert!(timer_content.contains("Unit=robertos-watching-backup.service"));
    assert!(timer_content.contains("WantedBy=timers.target"));
}

#[test]
fn test_env_example_production_paths_and_descriptions() {
    let env_content =
        fs::read_to_string(".env.example").expect(".env.example must exist in project root");

    assert!(env_content.contains("TELEGRAM_BOT_TOKEN="));
    assert!(env_content.contains("ADMIN_CHAT_ID="));
    assert!(env_content
        .contains("DATABASE_URL=sqlite:///opt/robertos-watching/robertos_watching.db?mode=rwc"));
    assert!(env_content.contains("MAX_USERS_DEFAULT=300"));
    assert!(env_content.contains("BOT_TZ=America/Managua"));
    assert!(env_content.contains("RUST_LOG=info"));
    assert!(env_content.contains("SCAN_INTERVAL_MIN=30"));
    assert!(env_content.contains("BACKUP_RETENTION_DAYS="));
    assert!(env_content.contains("chmod 600"));
}

#[test]
fn test_shell_scripts_syntax() {
    let status = Command::new("bash")
        .args(["-n", "deploy/install.sh", "deploy/backup.sh"])
        .status()
        .expect("bash must execute");
    assert!(
        status.success(),
        "deploy shell scripts must pass bash -n syntax check"
    );
}

#[test]
fn test_install_script_destdir_staging_and_permissions() {
    let temp_dir = TempTestDir::new("rw_install_test");
    let stage_dir = temp_dir.path.join("stage");
    let fake_bin = temp_dir.path.join("fake-bot");
    fs::write(&fake_bin, b"#!/bin/sh\necho bot").expect("write fake bin");
    fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).expect("chmod fake bin");

    let status = Command::new("bash")
        .arg("deploy/install.sh")
        .arg(&fake_bin)
        .env("DESTDIR", &stage_dir)
        .status()
        .expect("execute install.sh");

    assert!(status.success(), "install.sh with DESTDIR must succeed");

    let opt_dir = stage_dir.join("opt/robertos-watching");
    let backups_dir = opt_dir.join("backups");
    let bin_path = opt_dir.join("robertos-watching-bot");
    let backup_script = opt_dir.join("backup.sh");
    let env_example = opt_dir.join(".env.example");
    let systemd_dir = stage_dir.join("etc/systemd/system");

    assert!(opt_dir.is_dir());
    assert!(backups_dir.is_dir());
    assert!(bin_path.is_file());
    assert!(backup_script.is_file());
    assert!(env_example.is_file());

    assert_eq!(
        fs::metadata(&opt_dir).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(&backups_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&bin_path).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::metadata(&backup_script).unwrap().permissions().mode() & 0o777,
        0o750
    );
    assert_eq!(
        fs::metadata(&env_example).unwrap().permissions().mode() & 0o777,
        0o644
    );

    assert!(systemd_dir.join("robertos-watching.service").is_file());
    assert!(systemd_dir
        .join("robertos-watching-backup.service")
        .is_file());
    assert!(systemd_dir.join("robertos-watching-backup.timer").is_file());
}

#[test]
fn test_install_script_fails_fast_on_missing_binary() {
    let temp_dir = TempTestDir::new("rw_install_fail_test");
    let stage_dir = temp_dir.path.join("stage");
    let missing_bin = temp_dir.path.join("missing-bot");

    let status = Command::new("bash")
        .arg("deploy/install.sh")
        .arg(&missing_bin)
        .env("DESTDIR", &stage_dir)
        .status()
        .expect("execute install.sh");

    assert_eq!(
        status.code(),
        Some(2),
        "install.sh must exit with code 2 on missing binary input"
    );
}

#[test]
fn test_backup_script_execution_and_integrity_check() {
    let temp_dir = TempTestDir::new("rw_backup_test");
    let db_path = temp_dir.path.join("test_app.db");
    let backups_dir = temp_dir.path.join("backups");

    // Create a real SQLite database in WAL mode with sample data
    let sqlite_create = Command::new("sqlite3")
        .arg(&db_path)
        .arg("PRAGMA journal_mode=WAL; CREATE TABLE users (id INT, name TEXT); INSERT INTO users VALUES (1, 'Alice');")
        .status();

    if let Ok(s) = sqlite_create {
        if s.success() {
            let backup_status = Command::new("bash")
                .arg("deploy/backup.sh")
                .arg(&db_path)
                .arg(&backups_dir)
                .env("BACKUP_RETENTION_DAYS", "1")
                .status()
                .expect("execute backup.sh");

            assert!(
                backup_status.success(),
                "backup.sh must complete successfully"
            );

            let entries: Vec<_> = fs::read_dir(&backups_dir)
                .expect("read backups dir")
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "db"))
                .collect();

            assert_eq!(entries.len(), 1, "exactly one backup file must be created");
            let backup_file = &entries[0];
            assert_eq!(
                fs::metadata(backup_file).unwrap().permissions().mode() & 0o777,
                0o600,
                "backup file must have 0600 permissions"
            );

            // Verify backup content matches original
            let output = Command::new("sqlite3")
                .arg(backup_file)
                .arg("SELECT name FROM users WHERE id = 1;")
                .output()
                .expect("query backup database");

            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "Alice");
        }
    }
}
