# 登録repoのGit操作

residentの `git` は、操作者が名前で登録したGit repositoryだけを操作します。
シェルや任意のGit引数は受け付けません。既存のGit認証を利用し、新しい認証情報を発行しません。

```json
{"tools":{"git_repositories":[{"name":"kamimusuhi","path":"/absolute/path/to/repository","allow_write":true,"allow_push":false}]}}
```

個体の `runtime.json` の `tools.allowed` に `git` を追加します。
`path` は実際のGit repository rootです。配置用のコードコピーだけではGit履歴がないため、
正規の履歴を持つcloneを用意して登録します。既存のソースコピーや未コミット編集は上書きしません。

```json
{"name":"git","arguments":{"action":"list"}}
{"name":"git","arguments":{"action":"status","repository":"kamimusuhi"}}
{"name":"git","arguments":{"action":"diff","repository":"kamimusuhi","paths":["src/example.rs"]}}
{"name":"git","arguments":{"action":"add","repository":"kamimusuhi","paths":["src/example.rs"]}}
{"name":"git","arguments":{"action":"commit","repository":"kamimusuhi","paths":["src/example.rs"],"message":"fix: handle the observed case"}}
```

- `list`: 登録名と操作権限。内部pathは返しません。
- `status`, `diff`（`staged:true`も可）, `log`（`limit`1〜50、既定10）, `branches`: 読み取り。
- `create_branch`: branchを作成するだけでcheckoutしません。新規branchには通常 `codex/` を使います。
- `checkout`: cleanなtreeでのみbranchを切替。
- `add`, `commit`: 個別の明示ファイルを最大32個指定。directory全体は指定できません。
  commitはConventional Commits形式を必須とし、`git commit --only` で選択したファイルだけをcommitします。
  選択ファイルはworking treeの現在の内容を使い、他者の別ファイルのstaged変更は混ぜません。
- `fetch`, `pull`: 既存 `origin` のみ。pullはclean treeかつfast-forwardのみ。
- `push`: `allow_push:true` の場合のみ、現在branchを既存 `origin` へnon-forceでpush。
  外部公開・pushの実行はその操作へのユーザー承認がある場合に限ります。機能の検証はlocal bare fixtureで行います。

mutating actionは `allow_write:true` が必要です。force、reset、clean、remote変更、任意URL、
任意オプション、shell、hook、外部diff/textconv、clean/smudge/process filterは利用できません。
checkout/pullは既存の編集を保全して拒否します。名前付きrepo以外やrepository内subdirectoryは拒否します。

`.git`、`.env*`、private、secret類、token/key/pem等の認証ファイルは内容参照・書き込みを拒否します。
statusはファイル名だけを示します。既定diffは認証pathを除外します。diff/logのcredentialに見える行・
既知prefixは結果から秘匿しますが、任意のソースに埋め込まれた全秘密を検出するDLPではありません。
認証情報をsourceやcommit messageへ入れないでください。

子プロセスは最小限の環境で起動し、local action60秒/network action120秒、結果32KiBに制限します。
network Gitの生stdout/stderrやremote URLは返しません。既存credential helperはnetwork認証のため利用します。
実Gitの失敗詳細は操作者がそのrepoで確認してください。tool path検証はOS sandboxではありません。

2026-11-06にMac/Piへ反映し、稼働APIで読み書きと既存originへのfetchを確認済みです。
[稼働検証記録](verification/git-service-controls-2026-11-06.md)を参照してください。
