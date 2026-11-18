//! Disputes: shown with every alternative, flagged where an assistant reads, resolved by the person on one node and
//! seen alike on the others through the ordinary log.

mod common;

use common::{Res, Temp, sp};
use memcp_core::access::{self, Caller, Kind, Rights};
use memcp_core::facts::{self, NewFact, Remembered};
use memcp_core::sync::{self, Cursor};
use memcp_core::text::Language;
use memcp_core::tools::{self, ToolError};
use memcp_core::users::OWNER;
use memcp_core::{Brain, Space};
use serde_json::{Value, json};

type Boxed = Box<dyn std::error::Error>;

fn notes() -> Space {
    sp("notes")
}

fn node(t: &Temp, name: &str) -> Result<Brain, Boxed> {
    Ok(Brain::init(
        &t.path().join(name),
        &[notes()],
        Language::English,
    )?)
}

fn write(b: &Brain, text: &str) -> Result<String, Boxed> {
    let conn = b.open_space(&notes())?;
    match facts::remember(&conn, &notes(), "me", NewFact::agent(text, "agent:t"), &[])? {
        Remembered::Created(f) => Ok(f.id),
        other => Err(format!("not created: {other:?}").into()),
    }
}

fn replace(b: &Brain, id: &str, text: &str) -> Result<String, Boxed> {
    let conn = b.open_space(&notes())?;
    Ok(facts::supersede(
        &conn,
        &notes(),
        "me",
        id,
        NewFact::agent(text, "agent:t"),
        None,
    )?
    .id)
}

/// Everything `from` holds that `to` lacks.
fn catch_up(from: &Brain, to: &Brain, from_name: &str, to_name: &str) -> Res {
    loop {
        let (pulled, _) = sync::cursors(to, from_name, &notes())?;
        let page = sync::events_after(from, &notes(), pulled, 50, 1 << 20, Some(to_name))?;
        let r = sync::apply(to, &notes(), &page.events, from_name)?;
        if r.stopped.is_some() || !r.rejected.is_empty() {
            return Err(format!("{:?} {:?}", r.stopped, r.rejected).into());
        }
        sync::set_cursor(to, from_name, &notes(), Cursor::Pulled, page.next)?;
        if !page.more {
            return Ok(());
        }
    }
}

/// Every node catches up with every other, twice, so what one learned from another travels on.
fn mesh(nodes: &[(&Brain, &str)]) -> Res {
    for _ in 0..2 {
        for (a, an) in nodes {
            for (b, bn) in nodes {
                if an != bn {
                    catch_up(a, b, an, bn)?;
                }
            }
        }
    }
    Ok(())
}

type Row = (String, String, Option<String>);

fn state(b: &Brain) -> Result<Vec<Row>, Boxed> {
    let conn = b.open_space(&notes())?;
    let mut st = conn.prepare("SELECT id, status, replaced_by FROM facts ORDER BY id")?;
    Ok(st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?)
}

fn active(b: &Brain) -> Result<Vec<String>, Boxed> {
    Ok(state(b)?
        .into_iter()
        .filter(|(_, s, _)| s == "active")
        .map(|(id, _, _)| id)
        .collect())
}

/// The page of the person: a console connection with their own spaces.
fn page(_b: &Brain) -> Caller {
    Caller {
        id: "page".into(),
        agent: "page".into(),
        user_id: OWNER.into(),
        all_spaces: false,
        known: vec![notes()],
        grants: [(notes(), access::RW)].into_iter().collect(),
        paths: Default::default(),
        read_only_forced: false,
        no_person_guide: true,
        rate_per_min: access::DEFAULT_RATE_PER_MIN,
        max_text: access::DEFAULT_MAX_TEXT,
        kind: Kind::Console,
        local: false,
    }
}

/// A connection made by the person for an assistant, with these rights.
fn assistant(b: &Brain, name: &str, spec: &str) -> Result<Caller, Boxed> {
    let ctl = b.open_control()?;
    let tok = access::connect_for(&ctl, OWNER, name, &Rights::parse(spec)?, None, false, false)?;
    Ok(access::authenticate(&ctl, &tok, 1_900_000_000)?.ok_or("refused")?)
}

fn call(b: &Brain, c: &Caller, name: &str, args: Value) -> Result<Value, ToolError> {
    tools::call(b, c, name, &args)
}

/// Three nodes, the same note replaced three ways.
struct Three {
    _t: Temp,
    a: Brain,
    b: Brain,
    c: Brain,
    base: String,
    on: [String; 3],
}

fn three() -> Result<Three, Boxed> {
    let t = Temp::new("disputes-three");
    let (a, b, c) = (node(&t, "a")?, node(&t, "b")?, node(&t, "c")?);
    let base = write(&a, "The meeting is on Monday")?;
    mesh(&[(&a, "a"), (&b, "b"), (&c, "c")])?;
    let on = [
        replace(&a, &base, "The meeting is on Tuesday")?,
        replace(&b, &base, "The meeting is on Friday")?,
        replace(&c, &base, "The meeting is on Sunday")?,
    ];
    mesh(&[(&a, "a"), (&b, "b"), (&c, "c")])?;
    Ok(Three {
        _t: t,
        a,
        b,
        c,
        base,
        on,
    })
}

#[test]
fn a_note_replaced_on_three_nodes_shows_one_dispute_with_three_versions_everywhere() -> Res {
    let x = three()?;
    for n in [&x.a, &x.b, &x.c] {
        let r = call(n, &page(n), "disputes", json!({}))?;
        assert_eq!(r["count"], json!(1), "{r}");
        let d = &r["disputes"][0];
        assert_eq!(d["fact"]["id"], json!(x.base));
        assert_eq!(d["space"], json!("notes"));
        let mut texts: Vec<&str> = d["alternatives"]
            .as_array()
            .ok_or("no alternatives")?
            .iter()
            .map(|a| a["text"].as_str().unwrap_or_default())
            .collect();
        texts.sort();
        assert_eq!(
            texts,
            [
                "The meeting is on Friday",
                "The meeting is on Sunday",
                "The meeting is on Tuesday"
            ]
        );
        assert!(
            d["alternatives"]
                .as_array()
                .ok_or("no alternatives")?
                .iter()
                .all(|a| a["kind"] == "replacement" && a["author"] == "me" && a["at"].is_string())
        );
        assert!(
            r["message"]
                .as_str()
                .unwrap_or_default()
                .contains("never pick")
        );
    }
    Ok(())
}

#[test]
fn resolved_on_one_node_the_others_show_no_dispute_and_the_same_active_note() -> Res {
    let x = three()?;
    let keep = x.on[1].clone();
    let r = call(
        &x.b,
        &page(&x.b),
        "dispute_resolve",
        json!({ "id": x.base, "keep": keep }),
    )?;
    assert_eq!(r["result"], json!("resolved"));
    assert_eq!(r["kept"], json!(keep));
    assert_eq!(r["retired"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        call(&x.b, &page(&x.b), "disputes", json!({}))?["count"],
        json!(0)
    );
    mesh(&[(&x.a, "a"), (&x.b, "b"), (&x.c, "c")])?;
    for n in [&x.a, &x.b, &x.c] {
        assert_eq!(call(n, &page(n), "disputes", json!({}))?["count"], json!(0));
        assert!(sync::disputes(&n.open_space(&notes())?)?.is_empty());
        assert_eq!(active(n)?, vec![keep.clone()]);
        let conn = n.open_space(&notes())?;
        // What lost is retired, never erased, with the reason; the note in dispute now points at the one kept.
        for lost in x.on.iter().filter(|i| **i != keep) {
            let f = facts::read_fact(&conn, lost)?.ok_or("erased")?;
            assert_eq!(f.status, "retired");
            assert_eq!(
                f.status_reason.as_deref(),
                Some(format!("dispute resolved: kept {keep}").as_str())
            );
        }
        let base = facts::read_fact(&conn, &x.base)?.ok_or("erased")?;
        assert_eq!(base.replaced_by.as_deref(), Some(keep.as_str()));
    }
    assert_eq!(state(&x.a)?, state(&x.b)?);
    assert_eq!(state(&x.a)?, state(&x.c)?);
    // The resolution is an event of the history, logged once on each node.
    for n in [&x.a, &x.b, &x.c] {
        let conn = n.open_space(&notes())?;
        let k: i64 = conn.query_row(
            "SELECT count(*) FROM sync_log WHERE kind = 'dispute_resolved'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(k, 1);
    }
    Ok(())
}

#[test]
fn a_dispute_that_comes_after_the_resolution_is_shown_again() -> Res {
    let t = Temp::new("disputes-late");
    let (a, b, c) = (node(&t, "a")?, node(&t, "b")?, node(&t, "c")?);
    let base = write(&a, "The fence is green")?;
    mesh(&[(&a, "a"), (&b, "b"), (&c, "c")])?;
    let on_a = replace(&a, &base, "The fence is blue")?;
    let on_b = replace(&b, &base, "The fence is red")?;
    // c is off while a and b settle it.
    let on_c = replace(&c, &base, "The fence is white")?;
    mesh(&[(&a, "a"), (&b, "b")])?;
    call(
        &a,
        &page(&a),
        "dispute_resolve",
        json!({ "id": base, "keep": on_b }),
    )?;
    mesh(&[(&a, "a"), (&b, "b")])?;
    assert_eq!(
        call(&b, &page(&b), "disputes", json!({}))?["count"],
        json!(0)
    );
    mesh(&[(&a, "a"), (&b, "b"), (&c, "c")])?;
    for n in [&a, &b, &c] {
        let d = sync::disputes(&n.open_space(&notes())?)?;
        let mut want = vec![on_b.clone(), on_c.clone()];
        want.sort();
        assert_eq!(
            d,
            vec![(base.clone(), want)],
            "the resolved one stays resolved, the new one is shown"
        );
    }
    assert_eq!(state(&a)?, state(&c)?);
    assert!(!active(&a)?.contains(&on_a));
    Ok(())
}

#[test]
fn retired_on_one_node_and_corrected_on_another_resolves_either_way() -> Res {
    for keep_retired in [true, false] {
        let t = Temp::new("disputes-retired");
        let (a, b) = (node(&t, "a")?, node(&t, "b")?);
        let base = write(&a, "Call the plumber")?;
        mesh(&[(&a, "a"), (&b, "b")])?;
        let new = replace(&a, &base, "Call the electrician")?;
        facts::retire(&b.open_space(&notes())?, "me", &base, "not needed")?;
        mesh(&[(&a, "a"), (&b, "b")])?;
        let r = call(&b, &page(&b), "disputes", json!({}))?;
        let alts = r["disputes"][0]["alternatives"]
            .as_array()
            .ok_or("none")?
            .clone();
        let retired = alts
            .iter()
            .find(|a| a["kind"] == "retired")
            .ok_or("no retirement")?;
        assert_eq!(retired["reason"], json!("not needed"));
        let keep = if keep_retired {
            "retired".to_string()
        } else {
            new.clone()
        };
        call(
            &b,
            &page(&b),
            "dispute_resolve",
            json!({ "id": base, "keep": keep }),
        )?;
        mesh(&[(&a, "a"), (&b, "b")])?;
        for n in [&a, &b] {
            assert_eq!(call(n, &page(n), "disputes", json!({}))?["count"], json!(0));
            let want: Vec<String> = if keep_retired {
                vec![]
            } else {
                vec![new.clone()]
            };
            assert_eq!(active(n)?, want);
        }
        assert_eq!(state(&a)?, state(&b)?);
    }
    Ok(())
}

#[test]
fn resolving_with_an_alternative_that_is_not_one_is_refused_and_changes_nothing() -> Res {
    let x = three()?;
    let other = write(&x.a, "The car is in the garage")?;
    let before = state(&x.a)?;
    for keep in [other.as_str(), "retired", x.base.as_str()] {
        let e = call(
            &x.a,
            &page(&x.a),
            "dispute_resolve",
            json!({ "id": x.base, "keep": keep }),
        );
        assert!(
            matches!(e, Err(ToolError::Invalid(ref m)) if m.contains("not an alternative")),
            "{keep}: {e:?}"
        );
    }
    let e = call(
        &x.a,
        &page(&x.a),
        "dispute_resolve",
        json!({ "id": other, "keep": other }),
    );
    assert!(
        matches!(e, Err(ToolError::Invalid(ref m)) if m.contains("not in dispute")),
        "{e:?}"
    );
    assert_eq!(state(&x.a)?, before);
    assert_eq!(
        call(&x.a, &page(&x.a), "disputes", json!({}))?["count"],
        json!(1)
    );
    Ok(())
}

#[test]
fn only_the_person_resolves_an_assistant_on_their_word_and_never_a_machine_or_a_reader() -> Res {
    let x = three()?;
    let args = json!({ "id": x.base, "keep": x.on[0] });
    // A key that only reads sees the dispute, not the tool that resolves it.
    let reader = assistant(&x.a, "reader", "notes:r")?;
    assert_eq!(
        call(&x.a, &reader, "disputes", json!({}))?["count"],
        json!(1)
    );
    assert!(
        !tools::definitions(&reader)
            .iter()
            .any(|d| d.name == "dispute_resolve")
    );
    assert!(matches!(
        call(&x.a, &reader, "dispute_resolve", args.clone()),
        Err(ToolError::Unknown)
    ));
    // A machine of the person never resolves.
    let mut peer = assistant(&x.a, "other-machine", "all")?;
    peer.kind = Kind::Peer;
    assert!(
        !tools::definitions(&peer)
            .iter()
            .any(|d| d.name == "dispute_resolve")
    );
    assert!(call(&x.a, &peer, "dispute_resolve", args.clone()).is_err());
    // An assistant that may write is refused until it says the person chose.
    let helper = assistant(&x.a, "helper", "notes:rw")?;
    assert!(
        tools::definitions(&helper)
            .iter()
            .any(|d| d.name == "dispute_resolve")
    );
    let e = call(&x.a, &helper, "dispute_resolve", args.clone());
    assert!(
        matches!(e, Err(ToolError::Denied(ref m)) if m.contains("person")),
        "{e:?}"
    );
    let e = call(
        &x.a,
        &helper,
        "dispute_resolve",
        json!({ "id": x.base, "keep": x.on[0], "confirmed_by_person": false }),
    );
    assert!(matches!(e, Err(ToolError::Denied(_))), "{e:?}");
    assert_eq!(
        call(&x.a, &page(&x.a), "disputes", json!({}))?["count"],
        json!(1)
    );
    let mut ok = args.clone();
    ok["confirmed_by_person"] = json!(true);
    // The id of an alternative names its dispute too.
    ok["id"] = json!(x.on[2]);
    let r = call(&x.a, &helper, "dispute_resolve", ok)?;
    assert_eq!(r["fact"], json!(x.base));
    assert_eq!(active(&x.a)?, vec![x.on[0].clone()]);
    Ok(())
}

#[test]
fn recall_and_read_fact_flag_a_note_in_dispute_with_its_alternatives() -> Res {
    let x = three()?;
    let helper = assistant(&x.a, "helper", "notes:rw")?;
    let r = call(&x.a, &helper, "recall", json!({ "query": "meeting" }))?;
    let hits = r["results"].as_array().ok_or("no results")?;
    assert_eq!(hits.len(), 3, "{r}");
    let mut want = x.on.to_vec();
    want.sort();
    for h in hits {
        assert_eq!(h["disputed"], json!(true), "{h}");
        assert_eq!(h["dispute_of"], json!(x.base));
        assert_eq!(h["alternatives"], json!(want));
    }
    let one = call(&x.a, &helper, "read_fact", json!({ "id": x.base }))?;
    assert_eq!(one["fact"]["disputed"], json!(true));
    // A note outside any dispute carries no flag.
    let calm = write(&x.a, "The cat sleeps on the sofa")?;
    let f = call(&x.a, &helper, "read_fact", json!({ "id": calm }))?;
    assert!(f["fact"].get("disputed").is_none(), "{f}");
    // Once resolved, the flag is gone.
    call(
        &x.a,
        &page(&x.a),
        "dispute_resolve",
        json!({ "id": x.base, "keep": x.on[0] }),
    )?;
    let r = call(&x.a, &helper, "recall", json!({ "query": "meeting" }))?;
    assert!(
        r["results"]
            .as_array()
            .ok_or("none")?
            .iter()
            .all(|h| h.get("disputed").is_none())
    );
    Ok(())
}

#[test]
fn retiring_an_alternative_by_hand_ends_the_dispute_on_every_node() -> Res {
    let t = Temp::new("disputes-hand");
    let (a, b) = (node(&t, "a")?, node(&t, "b")?);
    let base = write(&a, "The key is under the mat")?;
    mesh(&[(&a, "a"), (&b, "b")])?;
    let on_a = replace(&a, &base, "The key is in the box")?;
    let on_b = replace(&b, &base, "The key is with the neighbour")?;
    mesh(&[(&a, "a"), (&b, "b")])?;
    facts::retire(&a.open_space(&notes())?, "me", &on_a, "wrong")?;
    assert!(sync::disputes(&a.open_space(&notes())?)?.is_empty());
    mesh(&[(&a, "a"), (&b, "b")])?;
    assert!(sync::disputes(&b.open_space(&notes())?)?.is_empty());
    assert_eq!(state(&a)?, state(&b)?);
    assert_eq!(active(&b)?, vec![on_b]);
    Ok(())
}
