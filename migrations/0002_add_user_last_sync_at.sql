-- Migración 0002: Persistencia del último escaneo interactivo (/sync) por estudiante
-- Permite que el cooldown anti-ansiedad sobreviva al reinicio del servicio (R-15, v1.1.0)
ALTER TABLE users ADD COLUMN last_sync_at TIMESTAMP DEFAULT NULL;
