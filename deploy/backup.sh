#!/usr/bin/env bash
# ==============================================================================
# Roberto's Watching — SQLite Online Backup Script
# ==============================================================================
# Realiza un respaldo consistente en caliente de la base de datos SQLite utilizando
# la API de backup online de sqlite3 (".backup"), garantizando compatibilidad total
# con transacciones WAL activas sin bloquear lecturas ni escrituras concurrentes.
# ==============================================================================

set -euo pipefail
umask 077

DB_PATH="${1:-}"
BACKUP_DIR="${2:-/opt/robertos-watching/backups}"
RETENTION_DAYS="${BACKUP_RETENTION_DAYS:-30}"

log() {
    printf "[%s] [BACKUP] %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$*"
}

err() {
    printf "[%s] [BACKUP-ERROR] %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$*" >&2
}

# 1. Verificar herramienta sqlite3
if ! command -v sqlite3 >/dev/null 2>&1; then
    err "El comando 'sqlite3' no está instalado en el sistema. Instale 'sqlite3' (ej. apt install sqlite3)."
    exit 1
fi

# 2. Resolver ruta de la base de datos si no fue suministrada explícitamente
if [[ -z "$DB_PATH" ]]; then
    if [[ -z "${DATABASE_URL:-}" && -f "/opt/robertos-watching/.env" ]]; then
        DATABASE_URL="$(grep -E '^[[:space:]]*DATABASE_URL=' /opt/robertos-watching/.env | head -n1 | cut -d= -f2- | tr -d '\"'\'' ')"
    fi
    if [[ -n "${DATABASE_URL:-}" ]]; then
        CLEAN_URL="${DATABASE_URL#sqlite://}"
        CLEAN_URL="${CLEAN_URL%%\?*}"
        if [[ -n "$CLEAN_URL" ]]; then
            if [[ "$CLEAN_URL" != /* ]]; then
                DB_PATH="/opt/robertos-watching/${CLEAN_URL}"
            else
                DB_PATH="$CLEAN_URL"
            fi
        fi
    fi
    DB_PATH="${DB_PATH:-/opt/robertos-watching/robertos_watching.db}"
fi

# 3. Leer días de retención desde .env si no está en el entorno
if [[ -f "/opt/robertos-watching/.env" && -z "${BACKUP_RETENTION_DAYS:-}" ]]; then
    ENV_RETENTION="$(grep -E '^[[:space:]]*BACKUP_RETENTION_DAYS=' /opt/robertos-watching/.env | head -n1 | cut -d= -f2- | tr -d '\"'\'' ')"
    if [[ -n "$ENV_RETENTION" && "$ENV_RETENTION" =~ ^[0-9]+$ ]]; then
        RETENTION_DAYS="$ENV_RETENTION"
    fi
fi

# 4. Verificar existencia de la base de datos
if [[ ! -f "$DB_PATH" ]]; then
    err "Base de datos no encontrada en '$DB_PATH'. Imposible realizar respaldo."
    exit 1
fi

# 5. Asegurar directorio de respaldos con permisos restringidos 0700
mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"
if [[ "$(id -u)" -eq 0 ]] && id -u robertos >/dev/null 2>&1; then
    chown robertos:robertos "$BACKUP_DIR"
fi

# 6. Generar nombre de archivo con timestamp seguro
TIMESTAMP="$(date '+%Y%m%d_%H%M%S')"
BACKUP_FILE="${BACKUP_DIR}/rw-${TIMESTAMP}.db"
TEMP_BACKUP="${BACKUP_FILE}.tmp"

# Limpiar archivo temporal en caso de salida prematura
trap 'rm -f "${TEMP_BACKUP:-}"' EXIT INT TERM

log "Iniciando respaldo online de '$DB_PATH' hacia '$BACKUP_FILE'..."

# 7. Ejecutar .backup hacia archivo temporal
sqlite3 "$DB_PATH" ".backup '${TEMP_BACKUP}'"

# 8. Validar integridad del respaldo
INTEGRITY_CHECK="$(sqlite3 "$TEMP_BACKUP" "PRAGMA integrity_check;")"
if [[ "$INTEGRITY_CHECK" != "ok" ]]; then
    err "Fallo de integridad en el archivo de respaldo: $INTEGRITY_CHECK"
    exit 2
fi

chmod 600 "$TEMP_BACKUP"
mv "$TEMP_BACKUP" "$BACKUP_FILE"
chmod 600 "$BACKUP_FILE"
trap - EXIT INT TERM

if [[ "$(id -u)" -eq 0 ]] && id -u robertos >/dev/null 2>&1; then
    chown robertos:robertos "$BACKUP_FILE"
fi

BACKUP_SIZE="$(du -h "$BACKUP_FILE" | cut -f1)"
log "Respaldo completado exitosamente: '$BACKUP_FILE' (Tamaño: $BACKUP_SIZE, Integridad: $INTEGRITY_CHECK)"

# 9. Política de retención: purgar respaldos más antiguos que RETENTION_DAYS
if [[ "$RETENTION_DAYS" =~ ^[0-9]+$ && "$RETENTION_DAYS" -gt 0 ]]; then
    log "Purgando respaldos con más de $RETENTION_DAYS días de antigüedad en '$BACKUP_DIR'..."
    find "$BACKUP_DIR" -maxdepth 1 -type f -name "rw-*.db" -mtime +"$RETENTION_DAYS" -exec rm -f {} +
fi

# Limpieza preventiva de residuos temporales (.tmp) mayores a 1 día
find "$BACKUP_DIR" -maxdepth 1 -type f -name "rw-*.db.tmp" -mtime +1 -exec rm -f {} +

log "Operación de respaldo finalizada."
