"""Hard-delete Pasion users by username, walking FK children first (demo DBs only)."""
import sys, psycopg2
db, names = sys.argv[1], sys.argv[2:]
conn = psycopg2.connect(dbname=db, port=55438); conn.autocommit = False
cur = conn.cursor()
cur.execute("""select kcu.table_name, kcu.column_name, ccu.table_name, ccu.column_name
 from information_schema.table_constraints tc
 join information_schema.key_column_usage kcu on tc.constraint_name=kcu.constraint_name
 join information_schema.constraint_column_usage ccu on tc.constraint_name=ccu.constraint_name
 where tc.constraint_type='FOREIGN KEY' and tc.table_schema='public'""")
fks = cur.fetchall()
deleted = {}
def has_id(table):
    cur.execute("select 1 from information_schema.columns where table_schema='public' and table_name=%s and column_name='id'", (table,))
    return cur.fetchone() is not None
def purge(table, col, values, depth=0):
    if not values: return
    if not has_id(table):  # leaf table without a key: nothing can reference it
        cur.execute(f"delete from {table} where {col} = any(%s::uuid[])", (list(map(str, values)),))
        deleted[table] = deleted.get(table, 0) + cur.rowcount
        return
    cur.execute(f"select id from {table} where {col} = any(%s::uuid[])", (list(map(str, values)),))
    ids = [r[0] for r in cur.fetchall()]
    if not ids: return
    for (ct, cc, pt, pc) in fks:
        if pt != table or pc != 'id': continue
        if ct == table:  # self reference: unlink first
            cur.execute(f"update {table} set {cc}=null where {cc} = any(%s::uuid[])", (list(map(str, ids)),))
            continue
        purge(ct, cc, ids, depth+1)
    cur.execute(f"delete from {table} where id = any(%s::uuid[])", (list(map(str, ids)),))
    deleted[table] = deleted.get(table, 0) + cur.rowcount
cur.execute("select id, username from users where username = any(%s)", (names,))
users = cur.fetchall()
for uid, name in users:
    for (ct, cc, pt, pc) in fks:
        if pt == 'users' and pc == 'id':
            purge(ct, cc, [uid])
    cur.execute("delete from users where id=%s", (uid,)); deleted['users'] = deleted.get('users',0)+cur.rowcount
conn.commit()
print(db, "removed:", [n for _, n in users], deleted)
