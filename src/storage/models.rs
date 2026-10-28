use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

/// Estudiante registrado en el sistema.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct User {
    pub chat_id: i64,
    pub uam_cif: String,
    pub uam_user_id: i64,
    pub fullname: String,
    pub wstoken: String,
    pub is_active: bool,
    pub inactive_reason: Option<String>,
    pub summary_enabled: bool,
    pub summary_times: String,
    pub summary_days: String,
    pub created_at: Option<NaiveDateTime>,
}

/// Asignatura de UAM Virtual.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct Course {
    pub id: i64,
    pub fullname: String,
    pub shortname: Option<String>,
    pub last_scanned: Option<NaiveDateTime>,
}

/// Relación N:M de usuario matriculado en un curso.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct UserCourse {
    pub chat_id: i64,
    pub course_id: i64,
}

/// Tarea / asignación global de un curso.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct Assignment {
    pub id: i64,
    pub cmid: Option<i64>,
    pub course_id: i64,
    pub name: String,
    pub duedate: Option<i64>,
    pub cutoffdate: Option<i64>,
    pub last_updated_at: Option<NaiveDateTime>,
}

/// Estado individual de entrega y calificación por alumno y tarea.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct UserSubmission {
    pub chat_id: i64,
    pub assignment_id: i64,
    pub status: Option<String>,
    pub graded: bool,
    pub grade: Option<String>,
    pub feedback: Option<String>,
    pub due_override: Option<i64>,
    pub updated_at: Option<NaiveDateTime>,
}

/// Recurso o módulo semanal de un curso.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct Module {
    pub id: i64,
    pub course_id: i64,
    pub section_name: Option<String>,
    pub modname: Option<String>,
    pub name: String,
    pub url: Option<String>,
    pub created_at: Option<NaiveDateTime>,
}

/// Alerta emitida para control de idempotencia y prevención de spam.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct SentAlert {
    pub alert_key: String,
    pub alert_type: String,
    pub chat_id: Option<i64>,
    pub created_at: Option<NaiveDateTime>,
}

/// Token donante para rotación y failover en el escaneo compartido (PR #3).
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct DonorToken {
    pub chat_id: i64,
    pub token: String,
}

/// Configuración global persistida en base de datos.
#[derive(Debug, Clone, PartialEq, Eq, FromRow, Serialize, Deserialize)]
pub struct SystemSetting {
    pub key: String,
    pub value: String,
}
