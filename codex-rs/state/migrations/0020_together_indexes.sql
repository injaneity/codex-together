CREATE INDEX idx_together_servers_owner ON together_servers(owner_email, created_at DESC);
CREATE INDEX idx_together_members_role ON together_members(server_id, role, added_at DESC);
CREATE INDEX idx_together_thread_acl_owner ON together_thread_acl(server_id, owner_email, shared_at DESC);
CREATE INDEX idx_together_thread_forks_parent ON together_thread_forks(server_id, parent_thread_id, created_at ASC);
