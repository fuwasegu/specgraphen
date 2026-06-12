SELECT id, name, email FROM user WHERE id = ?;
UPDATE user SET email = ? WHERE id = ?;
INSERT INTO user (id, name, email) VALUES (?, ?, ?);
