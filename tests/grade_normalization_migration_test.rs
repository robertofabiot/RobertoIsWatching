use robertos_watching_bot::moodle::submissions::normalize_grade;
use sqlx::{Connection, Row, SqliteConnection};
use std::borrow::Cow;

/// La migración 0003 debe dejar las notas guardadas igual que `normalize_grade`; si difieren,
/// el escáner detecta un cambio de nota y reenvía la alerta de calificación.
#[tokio::test]
async fn test_migration_normalizes_stored_grades_like_client() {
    let mut conn = SqliteConnection::connect("sqlite::memory:")
        .await
        .expect("Fallo al abrir SQLite en memoria");
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut conn)
        .await
        .unwrap();

    // 1. Esquema anterior a la normalización
    let full = sqlx::migrate!("./migrations");
    let mut previous = sqlx::migrate!("./migrations");
    previous.migrations = Cow::Owned(
        full.migrations
            .iter()
            .filter(|m| m.version < 3)
            .cloned()
            .collect(),
    );
    previous.run(&mut conn).await.unwrap();

    // 2. Notas tal como las guardaba la versión anterior
    let raw_grades = [
        "0&nbsp;/&nbsp;30",
        "12&nbsp;/&nbsp;15",
        "10&#160;/&#160;10",
        "30 / 30",
    ];
    for (i, grade) in raw_grades.iter().enumerate() {
        sqlx::query(
            "INSERT INTO user_submissions (chat_id, assignment_id, graded, grade) VALUES (1, ?, 1, ?)",
        )
        .bind(i as i64)
        .bind(grade)
        .execute(&mut conn)
        .await
        .unwrap();
    }

    // 3. Aplicar la migración pendiente
    full.run(&mut conn).await.unwrap();

    for (i, grade) in raw_grades.iter().enumerate() {
        let row = sqlx::query("SELECT grade FROM user_submissions WHERE assignment_id = ?")
            .bind(i as i64)
            .fetch_one(&mut conn)
            .await
            .unwrap();
        let stored: String = row.get(0);
        assert_eq!(stored, normalize_grade(grade), "Nota original: {grade}");
    }
}
