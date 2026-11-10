import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('merge_history', Path(__file__).with_name('merge_conversation_history.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def turn(n='1', subject='eightman'):
    return dict(id='mac-'+n, node='mac', subject=subject, task_id='t'+n,
                turn_id='turn'+n, session_id='s1', individual_id='mac-individual',
                ts='2026-11-07T12:00:00Z', message='以前の好み', response='当時の返答',kind='answered')


def bundle(turns):
    return dict(schema=1, source_node='mac', source_individual_id='mac-individual', turns=turns)


class MigrationTests(unittest.TestCase):
    def test_received_is_folded_only_on_same_node_subject_task(self):
        answered=turn()
        received=dict(answered,id='intake',kind='received',response=None)
        other=dict(received,id='other',subject='someone-else')
        pending=dict(received,id='pending',task_id='pending-task')
        rows=m.folded([received,answered,answered,other,pending])
        self.assertEqual({r['id'] for r in rows},{answered['id'],'other','pending'})

    def test_same_id_different_data_aborts_without_overwrite(self):
        original=turn()
        conflict=dict(original,response='矛盾する返答')
        with self.assertRaises(ValueError):m.unique_records([original,conflict])
        with self.assertRaises(ValueError):m.prepare(bundle([original]),[conflict])

    def test_same_content_distinct_turns_and_subjects_are_not_collapsed(self):
        original=turn()
        rows,docs,report=m.prepare(bundle([original,turn('2'),turn('3','other-subject')]),[])
        self.assertEqual(report['new_turns'],3)
        self.assertEqual(rows[2]['subject'],'other-subject')
        self.assertEqual(len({r['id'] for r in rows}),3)
        self.assertIn('過去資料',rows[0]['response'])
        self.assertEqual(json.loads(docs[0][1])['user_text'],original['message'])
        self.assertTrue(json.loads(docs[0][1])['provenance']['historical_only'])

    def test_native_duplicate_and_already_imported_turn_are_skipped(self):
        t=turn()
        self.assertEqual(m.prepare(bundle([t]),[dict(t,id='another-record-id')])[2],
                         {'new_turns':0,'duplicates_skipped':1})
        rows,_,_=m.prepare(bundle([t]),[])
        self.assertEqual(m.prepare(bundle([t]),rows)[2]['new_turns'],0)
        t['message']='改変された原文'
        with self.assertRaises(ValueError):m.prepare(bundle([t]),rows)

    def test_apply_rerun_does_not_rewrite_any_file(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); journal=root/'spool/pi/2026-11-08-mac-history.jsonl'; archive=root/'archive'
            b=bundle([turn()])
            self.assertEqual(m.apply(b,[root/'spool'],journal,archive)['new_turns'],1)
            before={p.relative_to(root):(p.read_bytes(),p.stat().st_mtime_ns) for p in root.rglob('*') if p.is_file()}
            self.assertEqual(m.apply(b,[root/'spool'],journal,archive)['new_turns'],0)
            after={p.relative_to(root):(p.read_bytes(),p.stat().st_mtime_ns) for p in root.rglob('*') if p.is_file()}
            self.assertEqual(before,after)
            self.assertEqual(journal.stat().st_mode & 0o777,0o600)

    def test_rerun_detects_changed_archive_instead_of_claiming_success(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d);journal=root/'spool/pi/archive.jsonl';archive=root/'archive';b=bundle([turn()])
            m.apply(b,[root/'spool'],journal,archive)
            next(archive.glob('*.json')).write_text('corrupted')
            with self.assertRaises(ValueError):m.apply(b,[root/'spool'],journal,archive)

    def test_torn_journal_and_partial_archive_fail_closed(self):
        with tempfile.TemporaryDirectory() as d:
            root=Path(d); p=root/'x.jsonl';p.write_bytes(m.encoded(turn()))
            with self.assertRaises(ValueError):m.read_records([root])
            p.unlink(); archive=root/'archive';archive.mkdir()
            with self.assertRaises(ValueError):m.apply(bundle([turn()]),[root/'spool'],root/'new.jsonl',archive)
            self.assertFalse((root/'new.jsonl').exists())


if __name__=='__main__':unittest.main()
