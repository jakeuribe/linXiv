CREATE UNIQUE INDEX IF NOT EXISTS idx_project_share_id_unique ON PROJECT (SHARE_ID) WHERE STATUS != 'deleted'
