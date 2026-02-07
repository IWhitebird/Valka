-- Add output and error_message columns to tasks table for direct access
ALTER TABLE tasks ADD COLUMN output JSONB;
ALTER TABLE tasks ADD COLUMN error_message TEXT;
