# G0-v5 lineage

名称衝突により当初予定 G0-v4 → G0-v5 へ変更。

- 原依頼と事前照合: [PREFLIGHT_AUDIT.md](../g0_v4/PREFLIGHT_AUDIT.md)。保存済み原依頼は同ディレクトリの EXPERIMENT_INSTRUCTIONS.md。
- 歴史的G0-v4: `/Users/eightman/dev/sandbox/kamimusuhi-g0v4`, branch `exp/g0-v4-predictive-invariant`, commit `c44ac29`。その仕様・実装・FAIL判定・成果物を変更、上書き、再解釈しない。
- 新規G0-v5: `/Users/eightman/dev/sandbox/kamimusuhi/experiments/g0_v5/`。旧結果・checkpointをbaseline/resultとして利用しない。
- この会話の事前照合前に作成した未実行draft (`.local/g0-v4-preflight-drafts-20260913/`) は新規実装の出発点。歴史的G0-v4からのコード/checkpointコピーではなく、この会話で作成された未学習の草稿。データ設計はIID schedule、識別可能性、oracle分離について新たに改訂して固定する。
- 旧G0-v4のソースは差分の理解のためread-onlyで参照した。学習入力はobservationのみで、互換目的のaction入力や旧decoderを追加しない。
- C3 old-G0固定baselineはSKIP: 旧結果の流用禁止に加え旧encoderの入力契約は16次元/action、新実験は24次元/obs-only。変換器や旧checkpointで同等性を仮定しない。
