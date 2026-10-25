CREATE TABLE update_releases (
    app_id text NOT NULL,
    channel text NOT NULL,
    target text NOT NULL,
    release_id text NOT NULL,
    version text NOT NULL,
    sequence bigint NOT NULL CHECK (sequence > 0),
    bundle jsonb NOT NULL,
    bundle_hash text NOT NULL,
    lifecycle text NOT NULL DEFAULT 'draft' CHECK (lifecycle IN ('draft','published','disabled','revoked')),
    policy jsonb NOT NULL DEFAULT '{"percentage":100,"cohort_key":"initial","paused":false,"revision":0}',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, channel, release_id),
    UNIQUE (app_id, channel, target, version),
    UNIQUE (app_id, channel, target, sequence)
);
CREATE TABLE update_operations (
    app_id text NOT NULL, channel text NOT NULL, operation_id text NOT NULL,
    request_hash text NOT NULL, response jsonb NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id,channel,operation_id)
);
CREATE TABLE update_audit (
    id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    app_id text NOT NULL, channel text NOT NULL, release_id text NOT NULL,
    operation_id text NOT NULL, action text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (app_id,channel,operation_id)
);
CREATE FUNCTION update_metadata_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(NEW.app_id,NEW.channel,NEW.target,NEW.release_id,NEW.version,NEW.sequence,NEW.bundle,NEW.bundle_hash)
       IS DISTINCT FROM ROW(OLD.app_id,OLD.channel,OLD.target,OLD.release_id,OLD.version,OLD.sequence,OLD.bundle,OLD.bundle_hash) THEN
        RAISE EXCEPTION 'release metadata is immutable';
    END IF;
    IF OLD.lifecycle='revoked' AND NEW.lifecycle <> 'revoked' THEN RAISE EXCEPTION 'revocation is terminal'; END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER immutable_update_release BEFORE UPDATE ON update_releases FOR EACH ROW EXECUTE FUNCTION update_metadata_immutable();
