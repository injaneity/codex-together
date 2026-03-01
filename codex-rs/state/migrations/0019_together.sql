CREATE TABLE together_servers (
    server_id TEXT PRIMARY KEY,
    owner_email TEXT NOT NULL,
    public_base_url TEXT NOT NULL,
    invite_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    closed_at INTEGER
);

CREATE TABLE together_members (
    server_id TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    removed_at INTEGER,
    PRIMARY KEY (server_id, email),
    FOREIGN KEY(server_id) REFERENCES together_servers(server_id) ON DELETE CASCADE
);

CREATE TABLE together_thread_acl (
    server_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    owner_email TEXT NOT NULL,
    shared_by_email TEXT NOT NULL,
    shared_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, thread_id),
    FOREIGN KEY(server_id) REFERENCES together_servers(server_id) ON DELETE CASCADE
);

CREATE TABLE together_thread_forks (
    server_id TEXT NOT NULL,
    child_thread_id TEXT NOT NULL,
    parent_thread_id TEXT NOT NULL,
    actor_email TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (server_id, child_thread_id),
    FOREIGN KEY(server_id) REFERENCES together_servers(server_id) ON DELETE CASCADE
);

CREATE TABLE together_client_session (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    mode TEXT NOT NULL,
    server_id TEXT,
    owner_email TEXT,
    endpoint TEXT,
    checked_out_thread_id TEXT,
    host_pid INTEGER,
    updated_at INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);
