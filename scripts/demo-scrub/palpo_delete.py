"""Remove Palpo users and their per-user rows (demo DBs only); room/event history untouched."""
import sys, psycopg2
db, ids = sys.argv[1], sys.argv[2:]
KEEP = {'room_users','event_push_actions','event_push_summaries','event_receipts','event_reports','event_idempotents',
 'presence_recipient_sets','presence_recipient_streams','remote_device_list_versions','remote_presence_recipients',
 'delayed_events','outgoing_requests','room_invite_delivery_batches','room_typings','threepid_id_servers','user_profile_changes'}
conn = psycopg2.connect(dbname=db, port=55438); cur = conn.cursor()
cur.execute("select distinct table_name from information_schema.columns where table_schema='public' and column_name='user_id'")
removed = {}
for (t,) in cur.fetchall():
    if t in KEEP: continue
    cur.execute(f"delete from {t} where user_id = any(%s)", (ids,))
    if cur.rowcount: removed[t] = cur.rowcount
cur.execute("delete from users where id = any(%s)", (ids,)); removed['users'] = cur.rowcount
conn.commit(); print(db, removed)
