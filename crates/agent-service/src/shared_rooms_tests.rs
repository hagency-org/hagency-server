use super::*;
use diesel::OptionalExtension;

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_agent_additional_project_room_is_owner_bound_and_keeps_original_chat() {
    let (f, request, assignment, matrix) = prepared_pool(true).await;
    let space = "!agent-space:example.test";
    let room = "!team-room:example.test";
    let owner = &f.outsider.p;
    let puppet = f
        .domain
        .managed_agent_matrix(owner, &f.policy, &request.id)
        .await
        .unwrap()
        .unwrap()
        .puppet_mxid;
    let mut states = matrix.states.lock().unwrap();
    states.insert(space.into(),vec![
        json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),
        json!({"type":"m.room.member","state_key":owner.mxid,"content":{"membership":"join"}}),
        json!({"type":"m.room.power_levels","state_key":"","content":{"users":{owner.mxid.clone():100}}}),
        json!({"type":"m.space.child","state_key":room,"content":{"via":["example.test"]}}),
    ]);
    let mut events = states[&request.room_id].clone();
    events.retain(|e| e["state_key"] != puppet);
    events.push(json!({"type":"m.room.power_levels","state_key":"","content":{"users":{owner.mxid.clone():100,SERVICE:100}}}));
    events.push(json!({"type":"m.room.member","state_key":"@member:example.test","content":{"membership":"join"}}));
    states.insert(room.into(), events);
    drop(states);
    let sf = matrix
        .gateway
        .admin(space, &owner.mxid, None)
        .await
        .unwrap();
    let project = f
        .domain
        .register_project(owner, space, &sf, crate::api::now_ms())
        .await
        .unwrap();
    let rf = matrix
        .gateway
        .admin(room, &owner.mxid, Some(space))
        .await
        .unwrap();
    f.domain
        .register_room(owner, &project.id, room, &sf, &rf, crate::api::now_ms())
        .await
        .unwrap();
    let input = crate::domain::BindRoom {
        project_id: project.id.clone(),
        room_id: room.into(),
        idempotency_key: "add-team".into(),
    };
    assert!(
        f.domain
            .try_bind_shared_room(
                &f.owner.p,
                &assignment.agent_id,
                &input,
                &matrix.gateway,
                &matrix.matrix
            )
            .await
            .is_err(),
        "resource owner cannot bind someone else's Agent"
    );
    let added = f
        .domain
        .try_bind_shared_room(
            owner,
            &assignment.agent_id,
            &input,
            &matrix.gateway,
            &matrix.matrix,
        )
        .await
        .unwrap()
        .unwrap();
    for _ in 0..3 {
        f.domain
            .reconcile_shared_room(&added.binding.id, &matrix.gateway, &matrix.matrix)
            .await
            .unwrap();
    }
    let replay = f
        .domain
        .try_bind_shared_room(
            owner,
            &assignment.agent_id,
            &input,
            &matrix.gateway,
            &matrix.matrix,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.binding.id, added.binding.id);
    assert_eq!(replay.binding.state, "active");
    assert!(
        f.domain
            .room_agents(owner, crate::api::now_ms())
            .await
            .unwrap()
            .iter()
            .any(|a| a.id == assignment.agent_id)
    );
    assert_eq!(
        f.domain
            .pool_agent_request(owner, &request.id)
            .await
            .unwrap()
            .room_id,
        request.room_id
    );
    // A local Agent may bind to the same registered room without replacing the pooled one.
    let facts = matrix
        .gateway
        .room(room, space, &owner.mxid, None)
        .await
        .unwrap();
    let local = f
        .domain
        .create_bound_agent(
            owner,
            crate::domain::CreateBoundAgent {
                project_id: project.id,
                room_id: room.into(),
                display_name: "Local teammate".into(),
                idempotency_key: "local-teammate".into(),
            },
            &facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    member(
        &mut matrix.states.lock().unwrap(),
        room,
        &local.agent.puppet_mxid,
        "join",
    );
    let facts = matrix
        .gateway
        .room(room, space, &owner.mxid, Some(&local.agent.puppet_mxid))
        .await
        .unwrap();
    f.domain
        .activate_binding(
            owner,
            &local.binding.id,
            local.binding.generation,
            &facts,
            crate::api::now_ms(),
        )
        .await
        .unwrap();
    let roster=f.domain.room_roster(owner,&input.project_id,room,crate::api::now_ms()).await.unwrap();
    assert!(roster.iter().any(|a|a.agent_id==assignment.agent_id));
    assert!(roster.iter().any(|a|a.agent_id==local.agent.id));
    let runtime = shared_poll(&f).await;
    let activity = save(&f, &runtime, "qualified", 101)
        .await
        .unwrap()
        .activity
        .unwrap();
    let mut event = crate::transport::RoutedEvent {
        event_id: "$no-mention".into(),
        room_id: room.into(),
        sender_mxid: owner.mxid.clone(),
        body: "Hello".into(),
        mentioned_mxids: Default::default(),
        thread_root: None,
        encrypted: false,
        is_edit: false,
    };
    f.domain
        .route_shared_event(&f.policy, &event, crate::api::now_ms(), &matrix.gateway)
        .await
        .unwrap();
    assert!(
        f.domain
            .poll_shared_work(
                &f.owner.p,
                &f.policy,
                activity.authority.clone(),
                &matrix.gateway
            )
            .await
            .unwrap()
            .is_none()
    );
    event.event_id = "$group-mention".into();
    event.sender_mxid = "@member:example.test".into();
    event.mentioned_mxids.insert(puppet.clone());
    f.domain
        .route_shared_event(&f.policy, &event, crate::api::now_ms(), &matrix.gateway)
        .await
        .unwrap();
    let work = f
        .domain
        .poll_shared_work(
            &f.owner.p,
            &f.policy,
            activity.authority.clone(),
            &matrix.gateway,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(work.room_id, room);
    assert_eq!(work.room_binding.as_ref().unwrap().id, added.binding.id);
    let input = SharedWorkInput {
        authority: activity.authority.clone(),
        work_id: work.id.clone(),
        work_digest: allocation_digest(&work).unwrap(),
    };
    f.domain
        .acknowledge_shared_work(&f.owner.p, &f.policy, input.clone(), &matrix.gateway)
        .await
        .unwrap();
    f.domain
        .start_shared_work(&f.owner.p, &f.policy, input.clone(), &matrix.gateway)
        .await
        .unwrap();
    matrix.states.lock().unwrap().get_mut(room).unwrap().push(json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}));
    let encrypted=crate::transport::RoutedEvent{event_id:"$extra-room-encrypted".into(),room_id:room.into(),sender_mxid:"@member:example.test".into(),body:String::new(),mentioned_mxids:Default::default(),thread_root:None,encrypted:true,is_edit:false};
    f.domain.route_shared_event(&f.policy,&encrypted,crate::api::now_ms(),&matrix.gateway).await.unwrap();
    let scopes=f.domain.shared_crypto_poll(&f.owner.p,None).await.unwrap();
    assert!(scopes["scopes"].as_array().unwrap().iter().any(|s|s["roomId"]==room&&s["events"].as_array().is_some_and(|e|!e.is_empty())));
    let confirm=json!({"assignmentId":assignment.id,"eventId":encrypted.event_id,"addressed":true,"mentioned":true});
    f.domain.shared_crypto_confirm(&f.owner.p,serde_json::from_value(confirm).unwrap(),&matrix.gateway).await.unwrap();
    member(
        &mut matrix.states.lock().unwrap(),
        room,
        &owner.mxid,
        "leave",
    );
    f.domain
        .reconcile_shared_room(&added.binding.id, &matrix.gateway, &matrix.matrix)
        .await
        .unwrap();
    assert!(
        f.domain
            .start_shared_work(&f.owner.p, &f.policy, input, &matrix.gateway)
            .await
            .is_err()
    );
    let history=f.domain.shared_intake_history(owner,&assignment.id,crate::management_shared_intake_types::SharedIntakeQuery{after:None}).await.unwrap();
    assert!(history.items.iter().any(|r|r.event_id==encrypted.event_id&&r.state=="cancelled"),"revocation cancels queued messages from the former room generation");
    assert_eq!(
        f.domain
            .managed_agent_matrix(owner, &f.policy, &request.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        "matrix_verified",
        "leaving an extra room preserves the original conversation"
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn pool_original_room_becomes_group_requires_mentions_even_from_owner() {
    let (f, request, assignment, matrix) = prepared_pool(true).await;
    member(
        &mut matrix.states.lock().unwrap(),
        &request.room_id,
        "@member:example.test",
        "join",
    );
    let puppet = f
        .domain
        .managed_agent_matrix(&f.outsider.p, &f.policy, &request.id)
        .await
        .unwrap()
        .unwrap()
        .puppet_mxid;
    for (i, sender) in [&f.outsider.p.mxid, "@member:example.test"]
        .into_iter()
        .enumerate()
    {
        let mut event = crate::transport::RoutedEvent {
            event_id: format!("$plain{i}"),
            room_id: request.room_id.clone(),
            sender_mxid: sender.to_owned(),
            body: "Hello".into(),
            mentioned_mxids: Default::default(),
            thread_root: Some("$old-thread".into()),
            encrypted: false,
            is_edit: false,
        };
        f.domain
            .route_shared_event(&f.policy, &event, crate::api::now_ms(), &matrix.gateway)
            .await
            .unwrap();
        event.event_id = format!("$mentioned{i}");
        event.mentioned_mxids.insert(puppet.clone());
        f.domain
            .route_shared_event(&f.policy, &event, crate::api::now_ms(), &matrix.gateway)
            .await
            .unwrap();
    }
    let history = f
        .domain
        .shared_intake_history(
            &f.outsider.p,
            &assignment.id,
            crate::management_shared_intake_types::SharedIntakeQuery { after: None },
        )
        .await
        .unwrap();
    assert_eq!(history.items.len(), 2);
    assert!(
        history
            .items
            .iter()
            .all(|r| r.event_id.starts_with("$mentioned") && r.state == "pending")
    );
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn encrypted_pool_group_needs_explicit_mention_including_owner_and_threads() {
    let (f, request, assignment, matrix) = prepared_pool(true).await;
    {
        let mut states = matrix.states.lock().unwrap();
        member(
            &mut states,
            &request.room_id,
            "@member:example.test",
            "join",
        );
        states.get_mut(&request.room_id).unwrap().push(json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}));
    }
    for (index, sender) in [&f.outsider.p.mxid, "@member:example.test"]
        .into_iter()
        .enumerate()
    {
        for mentioned in [false, true] {
            let event = crate::transport::RoutedEvent {
                event_id: format!("$encrypted-{index}-{mentioned}"),
                room_id: request.room_id.clone(),
                sender_mxid: sender.to_owned(),
                body: String::new(),
                mentioned_mxids: Default::default(),
                thread_root: None,
                encrypted: true,
                is_edit: false,
            };
            f.domain
                .route_shared_event(&f.policy, &event, crate::api::now_ms(), &matrix.gateway)
                .await
                .unwrap();
            let input = json!({"assignmentId":assignment.id,"eventId":event.event_id,"addressed":true,"mentioned":mentioned,"threadRoot":"$existing-thread"});
            let result = f
                .domain
                .shared_crypto_confirm(
                    &f.owner.p,
                    serde_json::from_value(input).unwrap(),
                    &matrix.gateway,
                )
                .await
                .unwrap();
            assert_eq!(
                result["confirmed"], mentioned,
                "decryption or an existing thread must not authorize unmentioned group messages"
            );
        }
    }
    let history = f
        .domain
        .shared_intake_history(
            &f.outsider.p,
            &assignment.id,
            crate::management_shared_intake_types::SharedIntakeQuery { after: None },
        )
        .await
        .unwrap();
    assert_eq!(history.items.len(), 2);
}

#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn recovered_active_room_requires_fresh_membership_and_never_undoes_a_kick() {
    let (f, request, assignment, matrix) = prepared_pool(true).await;
    let owner=&f.outsider.p;
    let puppet=f.domain.managed_agent_matrix(owner,&f.policy,&request.id).await.unwrap().unwrap().puppet_mxid;
    let space="!recovered_space:example.test";
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    db.batch_execute("INSERT INTO hagency_agent_v1.projects(id,space_id,creation_policy) VALUES('recovered_project','!recovered_space:example.test','{}')").await.unwrap();
    for (name,requires_membership,present) in [("unobserved_kick",true,false),("still_member",true,true),("expired_cleanup",false,false)] {
        let room=format!("!{name}:example.test");
        {
            let mut states=matrix.states.lock().unwrap();
            let mut events=states[&request.room_id].clone();
            events.retain(|e|e["state_key"]!=puppet);
            events.push(json!({"type":"m.room.power_levels","state_key":"","content":{"users":{owner.mxid.clone():100,SERVICE:100}}}));
            if present {events.push(json!({"type":"m.room.member","state_key":puppet,"content":{"membership":"join"}}));}
            states.insert(room.clone(),events);
            states.entry(space.into()).or_insert_with(||vec![json!({"type":"m.room.create","state_key":"","content":{"type":"m.space"}}),json!({"type":"m.room.member","state_key":owner.mxid,"content":{"membership":"join"}})]).push(json!({"type":"m.space.child","state_key":room,"content":{"via":["example.test"]}}));
        }
        sql_query("INSERT INTO hagency_agent_v1.rooms(room_id,project_id,creation_policy) VALUES($1,'recovered_project','{}')").bind::<Text,_>(&room).execute(&mut db).await.unwrap();
        sql_query("INSERT INTO hagency_agent_v1.shared_agent_rooms(id,assignment_id,agent_id,project_id,room_id,state,restoration_requires_membership) VALUES($1,$2,$3,'recovered_project',$4,'joining',$5)").bind::<Text,_>(name).bind::<Text,_>(&assignment.id).bind::<Text,_>(&assignment.agent_id).bind::<Text,_>(&room).bind::<Bool,_>(requires_membership).execute(&mut db).await.unwrap();
        let calls=matrix.calls.lock().unwrap().len();
        f.domain.reconcile_shared_room(name,&matrix.gateway,&matrix.matrix).await.unwrap();
        #[derive(QueryableByName)]
        struct RoomState {#[diesel(sql_type=Text)] state:String,#[diesel(sql_type=diesel::sql_types::Nullable<Text>)] departure_reason:Option<String>}
        let result=sql_query("SELECT state,departure_reason FROM hagency_agent_v1.shared_agent_rooms WHERE id=$1").bind::<Text,_>(name).get_result::<RoomState>(&mut db).await.unwrap();
        match name {
            "unobserved_kick"=>{assert_eq!(result.state,"left");assert_eq!(result.departure_reason.as_deref(),Some("membership_removed"));assert_eq!(matrix.calls.lock().unwrap().len(),calls,"recovery must not reinvite a removed Agent");},
            "still_member"=>{assert_eq!(result.state,"active");assert_eq!(matrix.calls.lock().unwrap().len(),calls);},
            _=>{assert_eq!(result.state,"joining");assert_eq!(matrix.calls.lock().unwrap().len(),calls+1,"proven expiry cleanup can invite again");}
        }
    }
}


#[tokio::test]
#[ignore = "requires dedicated PostgreSQL database via HAGENCY_AGENT_TEST_DATABASE_URL"]
async fn encrypted_ingress_uses_its_source_clock_without_rewriting_received_time() {
    let (f,request,assignment,matrix)=prepared_pool(true).await;
    matrix.states.lock().unwrap().get_mut(&request.room_id).unwrap().push(json!({"type":"m.room.encryption","state_key":"","content":{"algorithm":"m.megolm.v1.aes-sha2"}}));
    let received=now_ms();
    let event=crate::transport::RoutedEvent{event_id:"$clock-skew-ciphertext".into(),room_id:request.room_id.clone(),sender_mxid:f.outsider.p.mxid.clone(),body:String::new(),mentioned_mxids:Default::default(),thread_root:None,encrypted:true,is_edit:false};
    // Reproduce the measured Docker PostgreSQL clock lag deterministically.
    f.domain.set_management_test_clock(received-1).await.unwrap();
    f.domain.route_shared_event(&f.policy,&event,received,&matrix.gateway).await.unwrap();
    f.domain.reset_management_test_clock().await.unwrap();
    let mut db=AsyncPgConnection::establish(&f.url).await.unwrap();
    #[derive(QueryableByName)] struct Received{#[diesel(sql_type=BigInt)] received_at_ms:i64}
    let saved=sql_query("SELECT received_at_ms FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE assignment_id=$1 AND event_id=$2").bind::<Text,_>(&assignment.id).bind::<Text,_>(&event.event_id).get_result::<Received>(&mut db).await.unwrap();
    assert_eq!(saved.received_at_ms,received,"never restamp old messages across a control or quota boundary");
    let input=json!({"assignmentId":assignment.id,"eventId":event.event_id,"addressed":true,"mentioned":false});
    assert_eq!(f.domain.shared_crypto_confirm(&f.owner.p,serde_json::from_value(input).unwrap(),&matrix.gateway).await.unwrap()["confirmed"],true);
    let future=crate::transport::RoutedEvent{event_id:"$future-ciphertext".into(),..event};
    f.domain.route_shared_event(&f.policy,&future,now_ms()+60_000,&matrix.gateway).await.unwrap();
    assert!(sql_query("SELECT received_at_ms FROM hagency_agent_v1.management_shared_e2ee_candidates WHERE assignment_id=$1 AND event_id=$2").bind::<Text,_>(&assignment.id).bind::<Text,_>(&future.event_id).get_result::<Received>(&mut db).await.optional().unwrap().is_none(),"genuinely future ingress is still rejected");
}
