-- Estudiantes registrados en la plataforma
CREATE TABLE IF NOT EXISTS users (
    chat_id INTEGER PRIMARY KEY,
    uam_cif TEXT UNIQUE NOT NULL,
    uam_user_id INTEGER NOT NULL,
    fullname TEXT NOT NULL,
    wstoken TEXT NOT NULL,
    is_active BOOLEAN DEFAULT 1,
    inactive_reason TEXT,
    summary_enabled BOOLEAN DEFAULT 1,
    summary_times TEXT DEFAULT '["07:30", "20:00"]',
    summary_days TEXT DEFAULT '[1, 2, 3, 4, 5]',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Asignaturas existentes en UAM Virtual
-- Nota (A-03): last_scanned nace NULL para distinguir cursos nunca escaneados
CREATE TABLE IF NOT EXISTS courses (
    id INTEGER PRIMARY KEY,
    fullname TEXT NOT NULL,
    shortname TEXT,
    last_scanned TIMESTAMP DEFAULT NULL
);

-- Relación N:M de asignaturas matriculadas por estudiante
CREATE TABLE IF NOT EXISTS user_courses (
    chat_id INTEGER NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
    course_id INTEGER NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    PRIMARY KEY (chat_id, course_id)
);
CREATE INDEX IF NOT EXISTS idx_user_courses_course ON user_courses(course_id);

-- Asignaciones / Tareas generales del curso
CREATE TABLE IF NOT EXISTS assignments (
    id INTEGER PRIMARY KEY,
    cmid INTEGER,
    course_id INTEGER NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    duedate INTEGER,
    cutoffdate INTEGER,
    last_updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_assignments_course_due ON assignments(course_id, duedate);

-- Estado individual de entrega y nota por cada alumno
-- Nota (A-07): due_override almacena extensión individual de plazo
CREATE TABLE IF NOT EXISTS user_submissions (
    chat_id INTEGER NOT NULL REFERENCES users(chat_id) ON DELETE CASCADE,
    assignment_id INTEGER NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    status TEXT DEFAULT 'pending',
    graded BOOLEAN DEFAULT 0,
    grade TEXT,
    feedback TEXT,
    due_override INTEGER,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (chat_id, assignment_id)
);

-- Materiales y recursos semanales por curso
CREATE TABLE IF NOT EXISTS modules (
    id INTEGER PRIMARY KEY,
    course_id INTEGER NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    section_name TEXT,
    modname TEXT,
    name TEXT NOT NULL,
    url TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Registro de auditoría de alertas emitidas (Idempotencia Anti-Spam por Alumno)
-- Nota (A-14): chat_id para retención y purga al hacer logout
CREATE TABLE IF NOT EXISTS sent_alerts (
    alert_key TEXT PRIMARY KEY,
    alert_type TEXT NOT NULL,
    chat_id INTEGER,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_sent_alerts_chat ON sent_alerts(chat_id);

-- Parámetros globales del sistema (Control de Cupos y Operación)
CREATE TABLE IF NOT EXISTS system_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Inicialización de parámetros por defecto (max_users se siembra dinámicamente en storage::init con MAX_USERS_DEFAULT)
INSERT OR IGNORE INTO system_settings (key, value) VALUES ('registration_open', 'true');

