CREATE TABLE user (
    id BIGINT NOT NULL COMMENT 'Primary key',
    name VARCHAR(100) NOT NULL COMMENT 'Display name',
    email VARCHAR(255) NOT NULL COMMENT 'Mail address',
    PRIMARY KEY (id)
);
