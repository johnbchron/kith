//! Integration tests for `SqliteStore` against an in-memory database.

use kith_core::{
  fact::{
    Confidence, ContactLabel, EmailValue, FactValue, NameValue, NewFact,
    RecordingContext,
  },
  store::ContactStore,
  subject::SubjectKind,
};
use uuid::Uuid;

use crate::SqliteStore;

async fn store() -> SqliteStore {
  SqliteStore::open_in_memory()
    .await
    .expect("in-memory store")
}

// ─── Subjects ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_and_get_subject() {
  let s = store().await;

  let subject = s.add_subject(SubjectKind::Person).await.unwrap();
  assert_eq!(subject.kind, SubjectKind::Person);

  let fetched = s.get_subject(subject.subject_id).await.unwrap();
  assert!(fetched.is_some());
  let fetched = fetched.unwrap();
  assert_eq!(fetched.subject_id, subject.subject_id);
  assert_eq!(fetched.kind, SubjectKind::Person);
}

#[tokio::test]
async fn get_subject_missing_returns_none() {
  let s = store().await;
  let result = s.get_subject(Uuid::new_v4()).await.unwrap();
  assert!(result.is_none());
}

#[tokio::test]
async fn list_subjects_all() {
  let s = store().await;
  s.add_subject(SubjectKind::Person).await.unwrap();
  s.add_subject(SubjectKind::Organization).await.unwrap();
  s.add_subject(SubjectKind::Person).await.unwrap();

  let all = s.list_subjects(None).await.unwrap();
  assert_eq!(all.len(), 3);
}

#[tokio::test]
async fn list_subjects_filtered_by_kind() {
  let s = store().await;
  s.add_subject(SubjectKind::Person).await.unwrap();
  s.add_subject(SubjectKind::Organization).await.unwrap();
  s.add_subject(SubjectKind::Person).await.unwrap();

  let people = s.list_subjects(Some(SubjectKind::Person)).await.unwrap();
  assert_eq!(people.len(), 2);
  assert!(people.iter().all(|p| p.kind == SubjectKind::Person));
}

// ─── Fact recording ──────────────────────────────────────────────────────────

fn name_fact(subject_id: Uuid) -> NewFact {
  NewFact::new(
    subject_id,
    FactValue::Name(NameValue {
      given:      Some("Alice".into()),
      family:     Some("Liddell".into()),
      additional: None,
      prefix:     None,
      suffix:     None,
      full:       "Alice Liddell".into(),
    }),
  )
}

fn email_fact(subject_id: Uuid, address: &str) -> NewFact {
  NewFact::new(
    subject_id,
    FactValue::Email(EmailValue {
      address:    address.into(),
      label:      ContactLabel::Work,
      preference: 1,
    }),
  )
}

#[tokio::test]
async fn record_fact_and_retrieve() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let fact = s.record_fact(name_fact(subject.subject_id)).await.unwrap();
  assert_eq!(fact.subject_id, subject.subject_id);

  // get_facts should return it as Active.
  let facts = s.get_facts(subject.subject_id, None, false).await.unwrap();
  assert_eq!(facts.len(), 1);
  assert!(facts[0].status.is_active());
  assert_eq!(facts[0].fact.fact_id, fact.fact_id);
}

#[tokio::test]
async fn record_multiple_facts() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  s.record_fact(name_fact(subject.subject_id)).await.unwrap();
  s.record_fact(email_fact(subject.subject_id, "alice@example.com"))
    .await
    .unwrap();
  s.record_fact(email_fact(subject.subject_id, "alice@work.example.com"))
    .await
    .unwrap();

  let facts = s.get_facts(subject.subject_id, None, false).await.unwrap();
  assert_eq!(facts.len(), 3);
  assert!(facts.iter().all(|rf| rf.status.is_active()));
}

#[tokio::test]
async fn fact_confidence_and_source_roundtrip() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let mut input = name_fact(subject.subject_id);
  input.confidence = Confidence::Probable;
  input.source = Some("LinkedIn scrape".into());
  input.tags = vec!["imported".into(), "unverified".into()];

  let fact = s.record_fact(input).await.unwrap();

  let facts = s.get_facts(subject.subject_id, None, false).await.unwrap();
  let rf = facts
    .into_iter()
    .find(|rf| rf.fact.fact_id == fact.fact_id)
    .unwrap();

  assert_eq!(rf.fact.confidence, Confidence::Probable);
  assert_eq!(rf.fact.source.as_deref(), Some("LinkedIn scrape"));
  assert_eq!(rf.fact.tags, &["imported", "unverified"]);
}

#[tokio::test]
async fn recording_context_imported_roundtrip() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let mut input = name_fact(subject.subject_id);
  input.recording_context = RecordingContext::Imported {
    source_name:  "Google Contacts 2024-01".into(),
    original_uid: Some("abc-123".into()),
  };

  let fact = s.record_fact(input).await.unwrap();
  let facts = s.get_facts(subject.subject_id, None, false).await.unwrap();
  let rf = facts
    .into_iter()
    .find(|rf| rf.fact.fact_id == fact.fact_id)
    .unwrap();

  assert!(matches!(
    rf.fact.recording_context,
    RecordingContext::Imported { ref source_name, ref original_uid }
      if source_name == "Google Contacts 2024-01" && original_uid.as_deref() == Some("abc-123")
  ));
}

// ─── Supersession ────────────────────────────────────────────────────────────

#[tokio::test]
async fn supersede_marks_old_inactive() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let old = s
    .record_fact(email_fact(subject.subject_id, "old@example.com"))
    .await
    .unwrap();
  let replacement = email_fact(subject.subject_id, "new@example.com");

  let (sup, new_fact) = s.supersede(old.fact_id, replacement).await.unwrap();

  assert_eq!(sup.old_fact_id, old.fact_id);
  assert_eq!(sup.new_fact_id, new_fact.fact_id);

  // Active-only view: only the new fact.
  let active = s.get_facts(subject.subject_id, None, false).await.unwrap();
  assert_eq!(active.len(), 1);
  assert_eq!(active[0].fact.fact_id, new_fact.fact_id);
  assert!(active[0].status.is_active());

  // Full history: both facts, old marked Superseded.
  let all = s.get_facts(subject.subject_id, None, true).await.unwrap();
  assert_eq!(all.len(), 2);

  let old_rf = all
    .iter()
    .find(|rf| rf.fact.fact_id == old.fact_id)
    .unwrap();
  assert!(
    matches!(old_rf.status, kith_core::lifecycle::FactStatus::Superseded { by, .. } if by == new_fact.fact_id)
  );
}

#[tokio::test]
async fn supersede_already_superseded_errors() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let old = s
    .record_fact(email_fact(subject.subject_id, "a@example.com"))
    .await
    .unwrap();
  s.supersede(old.fact_id, email_fact(subject.subject_id, "b@example.com"))
    .await
    .unwrap();

  let err = s
    .supersede(old.fact_id, email_fact(subject.subject_id, "c@example.com"))
    .await
    .unwrap_err();
  assert!(matches!(err, crate::Error::AlreadySuperseded(_)));
}

#[tokio::test]
async fn supersede_nonexistent_fact_errors() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let err = s
    .supersede(
      Uuid::new_v4(),
      email_fact(subject.subject_id, "a@example.com"),
    )
    .await
    .unwrap_err();
  assert!(matches!(err, crate::Error::FactNotFound(_)));
}

// ─── Retraction ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn retract_removes_from_active_view() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let fact = s
    .record_fact(email_fact(subject.subject_id, "gone@example.com"))
    .await
    .unwrap();
  let ret = s
    .retract(fact.fact_id, Some("wrong address".into()))
    .await
    .unwrap();

  assert_eq!(ret.fact_id, fact.fact_id);
  assert_eq!(ret.reason.as_deref(), Some("wrong address"));

  let active = s.get_facts(subject.subject_id, None, false).await.unwrap();
  assert!(active.is_empty());

  let all = s.get_facts(subject.subject_id, None, true).await.unwrap();
  assert_eq!(all.len(), 1);
  assert!(
    matches!(&all[0].status, kith_core::lifecycle::FactStatus::Retracted { reason, .. }
      if reason.as_deref() == Some("wrong address"))
  );
}

#[tokio::test]
async fn retract_already_retracted_errors() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let fact = s
    .record_fact(email_fact(subject.subject_id, "x@example.com"))
    .await
    .unwrap();
  s.retract(fact.fact_id, None).await.unwrap();

  let err = s.retract(fact.fact_id, None).await.unwrap_err();
  assert!(matches!(err, crate::Error::AlreadyRetracted(_)));
}

#[tokio::test]
async fn retract_nonexistent_fact_errors() {
  let s = store().await;
  let err = s.retract(Uuid::new_v4(), None).await.unwrap_err();
  assert!(matches!(err, crate::Error::FactNotFound(_)));
}

#[tokio::test]
async fn cannot_retract_superseded_fact() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let old = s
    .record_fact(email_fact(subject.subject_id, "a@example.com"))
    .await
    .unwrap();
  s.supersede(old.fact_id, email_fact(subject.subject_id, "b@example.com"))
    .await
    .unwrap();

  let err = s.retract(old.fact_id, None).await.unwrap_err();
  assert!(matches!(err, crate::Error::AlreadySuperseded(_)));
}

// ─── Materialize ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn materialize_returns_none_for_unknown_subject() {
  let s = store().await;
  let view = s.materialize(Uuid::new_v4(), None).await.unwrap();
  assert!(view.is_none());
}

#[tokio::test]
async fn materialize_active_facts() {
  let s = store().await;
  let subject = s.add_subject(SubjectKind::Person).await.unwrap();

  let name = s.record_fact(name_fact(subject.subject_id)).await.unwrap();
  let email = s
    .record_fact(email_fact(subject.subject_id, "alice@example.com"))
    .await
    .unwrap();
  let stale = s
    .record_fact(email_fact(subject.subject_id, "old@example.com"))
    .await
    .unwrap();
  s.supersede(
    stale.fact_id,
    email_fact(subject.subject_id, "newer@example.com"),
  )
  .await
  .unwrap();

  let view = s
    .materialize(subject.subject_id, None)
    .await
    .unwrap()
    .unwrap();

  // subject metadata matches
  assert_eq!(view.subject.subject_id, subject.subject_id);

  // only active facts included: name + email + newer (3), not stale
  assert_eq!(view.active_facts.len(), 3);
  assert!(view.active_facts.iter().all(|rf| rf.status.is_active()));

  // original facts are present
  let ids: Vec<_> =
    view.active_facts.iter().map(|rf| rf.fact.fact_id).collect();
  assert!(ids.contains(&name.fact_id));
  assert!(ids.contains(&email.fact_id));
  assert!(!ids.contains(&stale.fact_id));
}

// ─── Search ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn search_by_text() {
  let s = store().await;

  let alice = s.add_subject(SubjectKind::Person).await.unwrap();
  let bob = s.add_subject(SubjectKind::Person).await.unwrap();

  s.record_fact(email_fact(alice.subject_id, "alice@example.com"))
    .await
    .unwrap();
  s.record_fact(email_fact(bob.subject_id, "bob@example.com"))
    .await
    .unwrap();

  let results = s
    .search(&kith_core::store::FactQuery {
      text: Some("alice".into()),
      ..Default::default()
    })
    .await
    .unwrap();

  assert_eq!(results.len(), 1);
  assert_eq!(results[0].subject_id, alice.subject_id);
}

#[tokio::test]
async fn search_by_kind() {
  let s = store().await;

  s.add_subject(SubjectKind::Person).await.unwrap();
  s.add_subject(SubjectKind::Organization).await.unwrap();

  let results = s
    .search(&kith_core::store::FactQuery {
      kind: Some(SubjectKind::Organization),
      ..Default::default()
    })
    .await
    .unwrap();

  assert_eq!(results.len(), 1);
  assert_eq!(results[0].kind, SubjectKind::Organization);
}
