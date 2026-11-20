-- Migración 0003: Normalización de calificaciones guardadas con entidades HTML
-- Moodle entrega gradefordisplay como "0&nbsp;/&nbsp;30". El cliente ahora lo normaliza a
-- "0 / 30"; se reescriben las filas existentes con el mismo resultado para que el escáner
-- no las interprete como una nota nueva y reenvíe la alerta.
UPDATE user_submissions
SET grade = TRIM(REPLACE(REPLACE(REPLACE(grade, '&nbsp;', ' '), '&#160;', ' '), '&#xA0;', ' '))
WHERE grade LIKE '%&nbsp;%' OR grade LIKE '%&#160;%' OR grade LIKE '%&#xA0;%';
