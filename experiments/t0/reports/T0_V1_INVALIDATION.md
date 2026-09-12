# T0-v1 Protocol Invalidation Record

この文書は PR #28 初版 (T0-v1) の測定プロトコルに存在した defect を記録し、
v1 数値・解釈のうち scientific conclusion として無効なものを明確にする。
v1 artifact (`artifacts/primary/`, Git 管理外) と v1 レポート履歴は記録として
保持するが、**T0-v2 以降の判定には一切使用しない**。

## Defect 1 — Interpolation contamination

v1 training distribution:

```text
mixed:
  50% grid    = [8,16,24,32,48,64]
  50% uniform = every integer in [8,64]
```

v1 evaluation の interpolation delay は `[40,56]`。uniform branch は `[8,64]`
全区間を等確率で sampling するため、`40` と `56` は training 中に通常頻度で
出現していた (seed ごとの実測 sample にも出現が確認できる)。

したがって以下の v1 主張は **invalid**:

- `T0-H2 unseen interpolation PASS`
- 「未知 interval への interpolation を実証した」に類する文言
- 総合 PASS のうち H2 を根拠にした部分

v2 では `38–42` と `54–58` の band 全体を `excluded_training_delays` として
training support から除去し、単一点ではなく「未観測 interval 領域への補間」
を検証する。

## Defect 2 — Invalid T-C3 observation intervention

v1 の `obs_blank(0., 1.)` は `t ∈ [0, T*)` で観測を定数 `.5` に置き換え、
`t == T*` で解除していた。これは以下を引き起こす:

- initial cue (t < pulse_length) まで消失する
- `T*` まで観測が完全に constant
- `T*` で観測分布が復帰し、その **遷移自体が target-time cue になる**

v1 の GRU/LSTM の obs blank 全滅は「dynamics が時計を駆動する」証拠ではなく、
「定数化 + T* 解除」という artifact への応答と区別できない (実際、memoryless
MLP が blank 下で見かけ上改善したのはこの解除遷移を拾ったため)。

したがって v1 T-C3 からの以下の解釈を **撤回** する:

- world dynamics が timing を駆動しているという因果解釈
- observation blank による失敗が internal clock 不在を示すという解釈

v2 では `obs_blank` を primary causal test から除外し、target step を一切
参照しない `post_cue_blank` (T-C3a) と episode 固有 obs を凍結する
`freeze_dynamics` (T-C3b) に置き換える。

## Defect 3 — Evaluation delay が実際には適用されていなかった

v1 の `_delay_env()` は `delay` 引数を horizon と seed の決定にのみ使い、
環境には `delays=[delay]` も `delay_override` も設定しなかった。`collect()`
内の `env.reset()` がそのまま走るため、**全 eval row は要求 delay ではなく
training の mixed 分布から再 sampling された delay で測定されていた**
(実測確認: delay=128 を要求した eval env が `[8,64]` の delay を生成)。

影響:

- `seen` / `interpolation` / `extrapolation` の per-delay 表は全て同一の
  mixed 分布の測定であり、行ラベルは名目のみだった
- `80/96/128` の extrapolation は **実際には 64 を超える delay を一度も
  測定していない**
- v1 `T0-H3 extrapolation PASS (Strong)` は全額 invalid
- `seen` / `interpolation` の数値も、名目 delay に対応する値ではないため
  全て invalid

v2 では `eval_delay` / `eval_with_intervention` / probe が
`reset(delay_override=...)` を経由して delay を強制し、
`tests/test_train_eval.py::test_eval_delay_forces_requested_delay` と
`tests/test_protocol.py::test_eval_can_force_heldout_delay` で固定する。

## v1 数値の扱い

- 上記 defect により、v1 の **全 per-delay 数値と全仮説判定を invalid とする**
  (単に H2/T-C3 だけでなく、defect 3 が seen/extrapolation にも及ぶため)
- 「捏造・故意の誤り」ではなく protocol implementation bug として記録する
- v2 では training protocol 自体が変わるため、旧 checkpoint の再利用・
  旧 eval JSON の aggregate 混入は禁止 (spec §20)
- v1 artifact は `experiments/t0/artifacts/primary/` に保持 (Git 管理外)。
  v2 は `experiments/t0/artifacts/primary_v2/` を使用する
