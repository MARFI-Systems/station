-- Keep the enum addition in its own migration: PostgreSQL cannot safely use a
-- newly-added enum value in an index predicate until the adding transaction commits.
ALTER TYPE email_user_provider_enum ADD VALUE IF NOT EXISTS 'MICROSOFT';
