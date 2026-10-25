"""THROWAWAY filesystem qualification seams, restricted to explicit fixture roots."""
import hashlib
from contextlib import closing
import json
import os
from pathlib import Path
import shutil
import sqlite3


def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()


class OwnedFiles:
    def __init__(self, root, paths):
        self.root=Path(root).resolve()
        self.entries=[]
        for path in paths:
            path=Path(path)
            relative=path.relative_to(self.root)
            if path.is_symlink():entry={'path':str(relative),'kind':'symlink','target':os.readlink(path)}
            elif path.is_file():entry={'path':str(relative),'kind':'file','sha256':sha(path)}
            else:continue
            self.entries.append(entry)

    def remove(self):
        removed=[];retained=[]
        for entry in self.entries:
            relative=Path(entry['path'])
            if relative.is_absolute() or '..' in relative.parts:
                retained.append({'path':str(relative),'reason':'unsafe-path'});continue
            path=self.root/relative
            if not path.exists() and not path.is_symlink():continue
            if not path.parent.resolve().is_relative_to(self.root):
                retained.append({'path':str(relative),'reason':'escaped-parent'});continue
            if entry['kind']=='symlink':
                same=path.is_symlink() and os.readlink(path)==entry['target']
            else:
                same=not path.is_symlink() and path.is_file() and sha(path)==entry['sha256']
            if same:path.unlink();removed.append(str(relative))
            else:retained.append({'path':str(relative),'reason':'modified-or-replaced'})
        return {'removed':removed,'retained':retained}


def schema(path):
    with closing(sqlite3.connect('file:'+str(path)+'?mode=ro',uri=True)) as db:
        return db.execute('PRAGMA user_version').fetchone()[0]


def backup(source,destination):
    with closing(sqlite3.connect(source)) as src,closing(sqlite3.connect(destination)) as dst,src,dst:src.backup(dst)


def require_rollback_compatibility(database,release):
    expected=json.loads((release/'release.json').read_text())['schema']
    if schema(database)>expected:raise RuntimeError('rollback-incompatible-schema')


def restore_backup(database,source,release):
    # Caller must stop both units first. This fixture never operates on real data.
    require_rollback_compatibility(source,release)
    def entries(path):
        with closing(sqlite3.connect('file:'+str(path)+'?mode=ro',uri=True)) as db:
            row=db.execute('SELECT body FROM session WHERE id=1').fetchone()
        return json.loads(row[0])['queue'] if row else []
    before=entries(database);after=entries(source)
    retained={q['entryId'] for q in after}
    report={'lostOccurrences':[q['entryId'] for q in before if q['entryId'] not in retained],
            'beforeCount':len(before),'afterCount':len(after),'restoredSchema':schema(source)}
    pending=database.with_name(database.name+'.restore')
    shutil.copy2(source,pending)
    with pending.open('rb') as f:os.fsync(f.fileno())
    # No handles may remain; SQLite's old WAL must not replay over the recovered file.
    for suffix in ('-wal','-shm'):Path(str(database)+suffix).unlink(missing_ok=True)
    pending.replace(database)
    return report


def purge_fixture_credentials(address, runtime):
    """Actual private-bus item deletion; never queries or changes the host keyring."""
    if address!='unix:path='+str(Path(runtime)/'bus'):raise RuntimeError('not-private-fixture-bus')
    from gi.repository import Gio,GLib
    connection=Gio.DBusConnection.new_for_address_sync(address,Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT|Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,None,None)
    def call(path,interface,method,parameters,reply):
        return connection.call_sync('org.freedesktop.secrets',path,interface,method,parameters,GLib.VariantType.new(reply),Gio.DBusCallFlags.NO_AUTO_START,3000,None).unpack()
    unlocked,locked=call('/org/freedesktop/secrets','org.freedesktop.Secret.Service','SearchItems',GLib.Variant('(a{ss})',({'application':'qoda-lifecycle-fixture','account':'fictional'},)),'(aoao)')
    report={'deleted':0,'incomplete':bool(locked),'lockedItems':len(locked)}
    for path in unlocked:
        try:
            prompt=call(path,'org.freedesktop.Secret.Item','Delete',None,'(o)')[0]
            if prompt=='/':report['deleted']+=1
            else:report['incomplete']=True
        except GLib.Error:report['incomplete']=True
    connection.close_sync(None)
    return report
