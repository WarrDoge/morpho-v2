CREATE EXTENSION IF NOT EXISTS vector;

CREATE TABLE IF NOT EXISTS events (
    id          bigserial PRIMARY KEY,
    event_id    text UNIQUE NOT NULL,
    ts          timestamptz NOT NULL DEFAULT now(),
    type        text NOT NULL,
    source      text NOT NULL,
    session_id  text,
    payload     jsonb NOT NULL DEFAULT '{}'
);

CREATE OR REPLACE FUNCTION events_immutable() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'events are immutable';
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS events_no_update ON events;
CREATE TRIGGER events_no_update BEFORE UPDATE OR DELETE ON events
    FOR EACH ROW EXECUTE FUNCTION events_immutable();

CREATE TABLE IF NOT EXISTS memories (
    id                 text PRIMARY KEY,
    kind               text NOT NULL,
    summary            text NOT NULL,
    status             text NOT NULL DEFAULT 'candidate',
    importance         double precision NOT NULL DEFAULT 0.5,
    confidence         double precision NOT NULL DEFAULT 0.5,
    source_events      text[] NOT NULL DEFAULT '{}',
    evidence           text[] NOT NULL DEFAULT '{}',
    entity_ids         text[] NOT NULL DEFAULT '{}',
    access_count       integer NOT NULL DEFAULT 0,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    last_reinforced_at timestamptz,
    valid_from         timestamptz,
    valid_until        timestamptz,
    embedding          vector(1024),
    version            integer NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS beliefs (
    id                     text PRIMARY KEY,
    proposition            text NOT NULL,
    confidence             double precision NOT NULL DEFAULT 0.5,
    status                 text NOT NULL DEFAULT 'hypothesis',
    supporting_evidence    text[] NOT NULL DEFAULT '{}',
    contradicting_evidence text[] NOT NULL DEFAULT '{}',
    created_at             timestamptz NOT NULL DEFAULT now(),
    updated_at             timestamptz NOT NULL DEFAULT now(),
    last_reviewed          timestamptz,
    valid_from             timestamptz,
    valid_until            timestamptz,
    embedding              vector(1024),
    version                integer NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS goals (
    id          text PRIMARY KEY,
    description text NOT NULL,
    priority    double precision NOT NULL DEFAULT 0.5,
    status      text NOT NULL DEFAULT 'proposed',
    origin      text NOT NULL,
    parent_goal text,
    deadline    timestamptz,
    evidence    text[] NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    version     integer NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS predictions (
    id          text PRIMARY KEY,
    prediction  text NOT NULL,
    probability double precision NOT NULL,
    deadline    timestamptz,
    verified    boolean,
    verified_at timestamptz,
    evidence    text[] NOT NULL DEFAULT '{}',
    created_at  timestamptz NOT NULL DEFAULT now(),
    version     integer NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS entities (
    id         text PRIMARY KEY,
    name       text NOT NULL,
    kind       text NOT NULL,
    attributes jsonb NOT NULL DEFAULT '{}',
    evidence   text[] NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    version    integer NOT NULL DEFAULT 1
);
CREATE UNIQUE INDEX IF NOT EXISTS entities_name_kind ON entities (lower(name), kind);

CREATE TABLE IF NOT EXISTS entity_relationships (
    id          text PRIMARY KEY,
    src         text NOT NULL REFERENCES entities(id),
    rel         text NOT NULL,
    dst         text NOT NULL REFERENCES entities(id),
    confidence  double precision NOT NULL DEFAULT 0.5,
    evidence    text[] NOT NULL DEFAULT '{}',
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS self_state (
    id         integer PRIMARY KEY CHECK (id = 1),
    data       jsonb NOT NULL,
    version    integer NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS working_state (
    id         integer PRIMARY KEY CHECK (id = 1),
    data       jsonb NOT NULL,
    version    integer NOT NULL DEFAULT 1,
    updated_at timestamptz NOT NULL DEFAULT now()
);
INSERT INTO self_state (id, data) VALUES (1, '{"capabilities": [], "limitations": [], "current_objectives": [], "commitments": [], "recent_actions": [], "known_failures": [], "uncertainties": [], "predicted_future_states": []}') ON CONFLICT DO NOTHING;
INSERT INTO working_state (id, data) VALUES (1, '{"current_topic": null, "active_entities": [], "recent_events": [], "open_questions": [], "current_task": null, "current_constraints": []}') ON CONFLICT DO NOTHING;

CREATE TABLE IF NOT EXISTS agent_proposals (
    id           text PRIMARY KEY,
    agent        text NOT NULL,
    operation    text NOT NULL,
    target       text,
    payload      jsonb NOT NULL,
    evidence     text[] NOT NULL DEFAULT '{}',
    confidence   double precision NOT NULL,
    decision     text NOT NULL,
    reason       text,
    source_event text,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS state_transitions (
    id          bigserial PRIMARY KEY,
    proposal_id text NOT NULL REFERENCES agent_proposals(id),
    table_name  text NOT NULL,
    object_id   text NOT NULL,
    before      jsonb,
    after       jsonb,
    agent       text NOT NULL,
    event_id    text,
    ts          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS state_transitions_object ON state_transitions (object_id);

CREATE TABLE IF NOT EXISTS state_snapshots (
    id            serial PRIMARY KEY,
    ts            timestamptz NOT NULL DEFAULT now(),
    last_event_id bigint NOT NULL,
    data          jsonb NOT NULL
);

CREATE TABLE IF NOT EXISTS event_cursors (
    consumer      text PRIMARY KEY,
    last_event_id bigint NOT NULL DEFAULT 0,
    updated_at    timestamptz NOT NULL DEFAULT now()
);
