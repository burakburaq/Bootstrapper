GRANT SELECT, INSERT, UPDATE ON update_releases TO update_runtime;
GRANT SELECT, INSERT ON update_operations, update_audit TO update_runtime;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO update_runtime;
