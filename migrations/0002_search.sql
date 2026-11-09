-- Full-text index over the current version of each entry.
-- The trigram tokenizer handles Japanese and other text without word boundaries.
-- Rows are maintained by the application whenever content or tags change.
-- The rowid of each row is entries.seq.
CREATE VIRTUAL TABLE entry_fts USING fts5(
    title,
    summary,
    body,
    tags,
    tokenize = 'trigram'
);

INSERT INTO entry_fts (rowid, title, summary, body, tags)
SELECT e.seq, v.title, coalesce(v.summary, ''), v.body,
       coalesce((SELECT group_concat(tag, ' ') FROM entry_tags WHERE entry_id = e.id), '')
FROM entries e
JOIN entry_versions v ON v.entry_id = e.id AND v.version = e.current_version;
