# Project instructions

For UI changes, read [`docs/design.md`](docs/design.md) before editing and
keep that guide current when the product behavior or visual constraints change.
Use `iced_test` widget interactions and geometry assertions for visual behavior;
run the focused simulator tests together with the relevant persistence tests.
Preserve existing data and compatibility when changing persistence, and test
migrations against existing databases.

Database compatibility is a standing requirement. Starting with schema 5,
new builds must open databases from all previously supported releases through
automatic, transactional migrations. Preserve existing sessions, timer IDs,
events, links, names, notes, and recorded time. Keep original local timing when
reconciliation changes displayed totals. Test every supported migration path,
including reopening the migrated database and retaining unrelated data. Refuse
unknown future schemas without modifying them; never require users to discard
or manually recreate an existing supported database.
