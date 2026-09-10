"""Prespecified balanced history lengths, reconstructed from past real frames only."""
import argparse,bisect,copy,hashlib,json,random
from pathlib import Path
from .acquire import save,emit


def prepare(source,frames_path,output):
    if output.exists():raise FileExistsError('Policy dataset is immutable')
    rows=[json.loads(s) for s in source.read_text().splitlines()]
    frames=[json.loads(s) for s in frames_path.read_text().splitlines()]
    times=[f['timestamp'] for f in frames]
    if times!=sorted(times):raise ValueError('Frame timestamps not monotonic')
    blocks={}
    for row in rows:blocks.setdefault(row['block_id'],[]).append(row)
    for block_index,(bid,block) in enumerate(sorted(blocks.items())):
        if len(block)!=8:raise ValueError('Each planned block must contain exactly eight decisions')
        lengths=list(range(1,9));random.Random(110910+block_index).shuffle(lengths)
        for row,length in zip(sorted(block,key=lambda r:r['episode_id']),lengths):
            when=row['timestamp'];normal_end=bisect.bisect_right(times,when);stale_end=bisect.bisect_right(times,when-30)
            if min(normal_end,stale_end)<length:raise ValueError('Insufficient recorded history')
            seq=frames[normal_end-length:normal_end];stale=copy.deepcopy(frames[stale_end-length:stale_end])
            for f in stale:f['values'][18]=min(1.,f['values'][18]+max(0,when-f['timestamp'])/60.)
            row['body_sequence']=[f['values'] for f in seq];row['body_mask_sequence']=[f['mask'] for f in seq]
            row['stale_body_sequence']=[f['values'] for f in stale];row['stale_body_mask_sequence']=[f['mask'] for f in stale]
            row['provenance'].update(frame_ids=[f['frame_id'] for f in seq],stale_frame_ids=[f['frame_id'] for f in stale],stale_age_seconds=when-stale[-1]['timestamp'],history_preparation='one_of_each_length_1_to_8_per_block_fixed_before_outcome_analysis',normalization_identity=seq[-1]['normalization_identity'])
    rows.sort(key=lambda r:r['timestamp'])
    for row in rows:emit(output,row)
    identity=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
    save(output.with_suffix('.manifest.json'),dict(schema_version='k0-f-policy-preparation-v1',source_sha256=identity(source),frames_sha256=identity(frames_path),output_sha256=identity(output),source_code_sha256=identity(Path(__file__)),rows=len(rows),blocks=len(blocks),history_lengths=list(range(1,9)),feature_selection_uses_outcomes=False,original_recordings_preserved=True))


def main():
    p=argparse.ArgumentParser();p.add_argument('--source',required=True,type=Path);p.add_argument('--frames',required=True,type=Path);p.add_argument('--output',required=True,type=Path);a=p.parse_args();prepare(a.source,a.frames,a.output)

if __name__=='__main__':main()
