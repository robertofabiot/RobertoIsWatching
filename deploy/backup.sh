#!/usr/bin/env bash
# ==============================================================================
# Roberto's Watching — SQLite Online Backup Script
# ==============================================================================
# Realiza un respaldo consistente en caliente de la base de datos SQLite utilizando
# la API de backup online de sqlite3 (".backup"), garantizando compatibilidad total
# con transacciones WAL activas sin bloquear lecturas ni escrituras concurrentes.
# ==============================================================================

set -euo pipefail

DB_PATH="${1:-/opt/robertos-watching/robertos_watching.db}"
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

# 2. Verificar existencia de la base de datos
if [[ ! -f "$DB_PATH" ]]; then
    log "Base de datos no encontrada en '$DB_PATH'. Omitiendo respaldo."
    exit 0
fi

# 3. Asegurar directorio de respaldos con permisos restringidos
if [[ ! -d "$BACKUP_DIR" ]]; then
    mkdir -p "$BACKUP_DIR"
    chmod 700 "$BACKUP_DIR"
    if [[ "$(id -u)" -eq 0 ]] && id -u robertos >/dev/null 2>&1; then
        chown robertos:robertos "$BACKUP_DIR"
    fi
fi

# 4. Generar nombre de archivo con timestamp seguro
TIMESTAMP="$(date '+%Y%m%d_%H%M%S')"
BACKUP_FILE="${BACKUP_DIR}/rw-${TIMESTAMP}.db"
TEMP_BACKUP="${BACKUP_FILE}.tmp"

log "Iniciando respaldo online de '$DB_PATH' hacia '$BACKUP_FILE'..."

# 5. Ejecutar .backup hacia archivo temporal
sqlite3 "$DB_PATH" ".backup '${TEMP_BACKUP}'"

# 6. Validar integridad del respaldo
INTEGRITY_CHECK="$(sqlite3 "$TEMP_BACKUP" "PRAGMA integrity_check;")"
if [[ "$INTEGRITY_CHECK" != "ok" ]]; then
    err "Fallo de integridad en el archivo de respaldo: $INTEGRITY_CHECK"
    rm -f "$TEMP_BACKUP"
    exit 2
fi

mv "$TEMP_BACKUP" "$BACKUP_FILE"
chmod 600 "$BACKUP_FILE"
if [[ "$(id -u)" -eq 0 ]] && id -u robertos >/dev/null 2>&1; then
    chown robertos:robertos "$BACKUP_FILE"
fi

BACKUP_SIZE="$(du -h "$BACKUP_FILE" | cut -f1)"
log "Respaldo completado exitosamente: '$BACKUP_FILE' (Tamaño: $BACKUP_SIZE, Integridad: $INTEGRITY_CHECK)"

# 7. Política de retención: purgar respaldos más antiguos que RETENTION_DAYS
if [[ -n "$RETENTION_DAYS" && "$RETENTION_DAYS" -gt 0 ]]; then
    log "Purgando respaldos con más de $RETENTION_DAYS días de antigüedad en '$BACKUP_DIR'..."
    find "$BACKUP_DIR" -type f -name "rw-*.db" -mtime +"$RETENTION_DAYS" -exec rm -f {} +
fi

log "Operación de respaldo finalizada."
