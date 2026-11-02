use remote_ssh_brige_lib::{
    database::{AuditQuery, Database},
    model::Audit,
};

#[test]
fn time_range_filters_count_pages_and_export_across_dates_and_timezones() {
    let dir = std::env::temp_dir().join(format!("audit-time-test-{}", uuid::Uuid::new_v4()));
    {
        let mut db = Database::open(&dir).unwrap();
        for (index, time) in [
            "2026-10-30T15:59:59.999Z",
            "2026-10-31T00:00:00+08:00",
            "2026-11-01T12:00:00Z",
            "2026-11-02T23:59:59.999+08:00",
            "2026-11-03T00:00:00+08:00",
        ]
        .iter()
        .enumerate()
        {
            db.insert_audit(&Audit {
                id: index.to_string(),
                time: (*time).into(),
                server_id: "server".into(),
                server_name: "host".into(),
                source: "desktop".into(),
                kind: "exec".into(),
                detail: "echo hello".into(),
                success: true,
                duration_ms: 1,
                summary: "ok".into(),
                response: None,
            })
            .unwrap();
        }
        let mut query = AuditQuery {
            start_time: Some("2026-10-30T16:00:00.000Z".into()),
            end_time: Some("2026-11-02T16:00:00.000Z".into()),
            category: "exec".into(),
            search: "hello".into(),
            limit: 2,
            ..Default::default()
        };
        let page = db.audit_page(&query).unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(
            page.entries
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["3", "2"]
        );
        query.offset = 2;
        assert_eq!(db.audit_page(&query).unwrap().entries[0].id, "1");
        let mut output = Vec::new();
        db.export_audits(&query, &mut output).unwrap();
        let exported: Vec<Audit> = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            exported.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["3", "2", "1"]
        );
        query.start_time = None;
        assert_eq!(db.audit_page(&query).unwrap().total, 4);
        query.end_time = None;
        assert_eq!(db.audit_page(&query).unwrap().total, 5);
    }
    let target = dir.canonicalize().unwrap();
    assert!(target.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(target
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("audit-time-test-"));
    std::fs::remove_dir_all(target).unwrap();
}
