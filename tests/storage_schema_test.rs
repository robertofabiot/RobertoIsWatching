use robertos_watching_bot::config::Config;
use robertos_watching_bot::storage;
use sqlx::Row;
use std::collections::HashSet;

#[tokio::test]
async fn test_pragmas_and_schema_are_applied() {
    let rand_id: u64 = rand::random();
    let temp_dir = std::env::temp_dir();
    let db_path = temp_dir.join(format!("roberto_test_schema_{rand_id}.db"));
    let database_url = format!("sqlite://{}?mode=rwc", db_path.display());

    let config = Config {
        bot_token: "123456789:ABCdefGhIJKlmNoPQRsTUVwxyZ".to_string(),
        admin_chat_id: 123456789,
        database_url: database_url.clone(),
        max_users_default: 350,
        bot_tz: chrono_tz::America::Managua,
        rust_log: "info".to_string(),
        scan_interval_min: 30,
    };

    let pool = storage::init(&config)
        .await
        .expect("Error al inicializar la base de datos temporal");

    // 1. Verificar PRAGMA journal_mode = wal
    let row = sqlx::query("PRAGMA journal_mode")
        .fetch_one(&pool)
        .await
        .expect("Fallo al consultar PRAGMA journal_mode");
    let journal_mode: String = row.get(0);
    assert_eq!(
        journal_mode.to_lowercase(),
        "wal",
        "El modo de journal debe ser WAL"
    );

    // 2. Verificar PRAGMA foreign_keys = 1 (true)
    let row = sqlx::query("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("Fallo al consultar PRAGMA foreign_keys");
    let foreign_keys: i64 = row.get(0);
    assert_eq!(foreign_keys, 1, "Foreign keys deben estar activadas");

    // 3. Verificar existencia de las 8 tablas requeridas
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&pool)
        .await
        .expect("Fallo al listar tablas");
    let table_names: HashSet<String> = rows.into_iter().map(|r| r.get(0)).collect();

    let expected_tables = [
        "users",
        "courses",
        "user_courses",
        "assignments",
        "user_submissions",
        "modules",
        "sent_alerts",
        "system_settings",
    ];

    for table in expected_tables {
        assert!(
            table_names.contains(table),
            "Falta la tabla esperada '{table}' en la base de datos"
        );
    }

    // 4. Verificar existencia de los índices creados
    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'index'")
        .fetch_all(&pool)
        .await
        .expect("Fallo al listar índices");
    let index_names: HashSet<String> = rows.into_iter().map(|r| r.get(0)).collect();

    let expected_indexes = [
        "idx_user_courses_course",
        "idx_assignments_course_due",
        "idx_sent_alerts_chat",
    ];

    for idx in expected_indexes {
        assert!(
            index_names.contains(idx),
            "Falta el índice esperado '{idx}' en la base de datos"
        );
    }

    // 5. Verificar siembra de system_settings
    let reg_row = sqlx::query("SELECT value FROM system_settings WHERE key = 'registration_open'")
        .fetch_one(&pool)
        .await
        .expect("Fallo al consultar registration_open");
    let reg_open: String = reg_row.get(0);
    assert_eq!(reg_open, "true");

    let max_row = sqlx::query("SELECT value FROM system_settings WHERE key = 'max_users'")
        .fetch_one(&pool)
        .await
        .expect("Fallo al consultar max_users");
    let max_users: String = max_row.get(0);
    assert_eq!(max_users, "350");

    // Limpieza
    drop(pool);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
}
