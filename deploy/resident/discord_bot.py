#!/usr/bin/env python3
"""Operator-only Discord gateway; the resident owns all individual state."""

import asyncio
from collections import OrderedDict
from contextlib import suppress
from dataclasses import dataclass, field
import hashlib
import io
import json
import logging
import os
import re
import time

import aiohttp
import discord

LOG = logging.getLogger("kamimusuhi.discord")
RESIDENT_URL = "http://127.0.0.1:7860"
# Accepted by both the resident API and `kamimusuhi-runtime talk --subject`.
SUBJECT_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")
# `@澪 開始 t1a0… devin` / `@澪 取消 t1a0…` (the ID may come from the notice replied to).
COMMAND_RE = re.compile(
    r"(?P<verb>開始|取消|取り消し|start|cancel)"
    r"(?:\s+(?P<id>t[0-9a-f]{6,32}))?"
    r"(?:\s+(?P<executor>[A-Za-z0-9][A-Za-z0-9_.-]{0,63}))?",
    re.IGNORECASE)
MESSAGE_LIMIT = 1900


def snowflakes(value: str) -> frozenset[int]:
    parts = value.split(",")
    if not all(p.strip().isascii() and p.strip().isdigit() for p in parts):
        raise ValueError("Discord IDs must be a nonempty comma-separated list")
    ids = frozenset(int(p) for p in parts)
    if not all(0 < n < 2**64 for n in ids):
        raise ValueError("Discord IDs must be positive 64-bit integers")
    return ids


def one_of(env: str, allowed: frozenset[int], what: str) -> int | None:
    """An explicit ID from `env` (which must be allowed), else the only allowed one."""
    raw = os.environ.get(env, "").strip()
    if raw:
        ids = snowflakes(raw)
        if len(ids) != 1 or not ids <= allowed:
            raise ValueError(f"{env} must be one of the allowed {what}")
        return next(iter(ids))
    return next(iter(allowed)) if len(allowed) == 1 else None


@dataclass(frozen=True)
class Config:
    token: str = field(repr=False)
    guild_id: int
    users: frozenset[int]
    channels: frozenset[int]
    # The operator's own subject (the one the desktop uses, e.g. $USER), so
    # Discord turns share relationship memory and recall with other surfaces.
    operator_subject: str | None = None
    # Who is mentioned, and where, when a task needs the operator's decision.
    # None disables those notices.
    notify_user: int | None = None
    notify_channel: int | None = None

    @classmethod
    def from_env(cls):
        token = os.environ.get("DISCORD_BOT_TOKEN", "").strip()
        if not token:
            raise ValueError("DISCORD_BOT_TOKEN is required")
        guilds = snowflakes(os.environ.get("DISCORD_GUILD_ID", ""))
        if len(guilds) != 1:
            raise ValueError("DISCORD_GUILD_ID must contain one ID")
        users = snowflakes(os.environ.get("DISCORD_ALLOWED_USER_IDS", ""))
        channels = snowflakes(os.environ.get("DISCORD_ALLOWED_CHANNEL_IDS", ""))
        operator = os.environ.get("DISCORD_OPERATOR_SUBJECT", "").strip() or None
        if operator is not None:
            if not SUBJECT_RE.fullmatch(operator):
                raise ValueError("DISCORD_OPERATOR_SUBJECT must be 1-64 chars of [A-Za-z0-9._-]")
            if len(users) != 1:
                # Sharing one subject between people would merge their memories.
                raise ValueError("DISCORD_OPERATOR_SUBJECT requires exactly one allowed user")
        notify_user = notify_channel = None
        if os.environ.get("DISCORD_NOTIFY", "on").strip().lower() not in ("off", "0", "false", "no"):
            notify_user = one_of("DISCORD_NOTIFY_USER_ID", users, "user IDs")
            notify_channel = one_of("DISCORD_NOTIFY_CHANNEL_ID", channels, "channel IDs")
        return cls(token, next(iter(guilds)), users, channels, operator,
                   notify_user, notify_channel)

    @property
    def notifies(self):
        return self.notify_user is not None and self.notify_channel is not None

    def allows(self, message):
        return (message.guild is not None
                and message.guild.id == self.guild_id
                and message.channel.id in self.channels
                and message.author.id in self.users
                and not message.author.bot
                and message.webhook_id is None)

    def subject(self, message):
        if self.operator_subject is not None and message.author.id in self.users:
            return self.operator_subject
        return subject_for(message)


def subject_for(message):
    # Stable across restarts, isolated by guild/channel/user, within the API's 64 chars.
    key = f"{message.guild.id}:{message.channel.id}:{message.author.id}"
    return "discord-" + hashlib.sha256(key.encode("ascii")).hexdigest()[:48]


def clip(text, limit):
    text = " ".join(str(text or "").split())
    return text if len(text) <= limit else text[:limit - 1] + "…"


def operator_only(user):
    """Mentions limited to the operator: generated text can ping nobody else."""
    if user is None:
        return discord.AllowedMentions.none()
    return discord.AllowedMentions(everyone=False, roles=False, replied_user=False,
                                   users=[discord.Object(id=user)])


class ResidentError(Exception):
    pass


class Resident:
    def __init__(self, session, url=RESIDENT_URL):
        self.session = session
        self.url = url

    async def _json(self, method, path, body=None, total=30):
        async with self.session.request(
            method, self.url + path, json=body,
            timeout=aiohttp.ClientTimeout(total=total, connect=5),
            allow_redirects=False,
        ) as response:
            raw = bytearray()
            async for chunk in response.content.iter_chunked(65536):
                raw.extend(chunk)
                if len(raw) > 4 * 1024 * 1024:
                    raise ResidentError("response too large")
            try:
                return response.status, json.loads(raw)
            except (ValueError, UnicodeError):
                raise ResidentError("invalid JSON") from None

    async def talk(self, message, subject):
        """The turn's reply: `response` plus what it left running (`background`)."""
        # No automatic retry: the resident may already have recorded the turn.
        async with self.session.post(
            self.url + "/v1/kamimusuhi/talk",
            json={"message": message, "subject": subject},
            timeout=aiohttp.ClientTimeout(total=660, connect=5),
            allow_redirects=False,
        ) as response:
            if response.status != 200:
                raise ResidentError(f"HTTP {response.status}")
            raw = bytearray()
            async for chunk in response.content.iter_chunked(65536):
                raw.extend(chunk)
                if len(raw) > 1024 * 1024:
                    raise ResidentError("response too large")
            try:
                payload = json.loads(raw)
            except (ValueError, UnicodeError):
                raise ResidentError("invalid JSON") from None
            text = payload.get("response") if isinstance(payload, dict) else None
            if not isinstance(text, str) or not text.strip():
                raise ResidentError("missing response")
            background = payload.get("background")
            return {"response": text,
                    "background": background if isinstance(background, dict) else None}

    async def tasks(self):
        status, payload = await self._json("GET", "/v1/tasks")
        tasks = payload.get("tasks") if isinstance(payload, dict) else None
        if status != 200 or not isinstance(tasks, list):
            raise ResidentError(f"tasks HTTP {status}")
        return [t for t in tasks if isinstance(t, dict)]

    async def delegated(self):
        """Delegated tasks from the task plane (this node's or a peer's)."""
        status, payload = await self._json("POST", "/v1/agents", {"action": "panel"}, total=60)
        tasks = payload.get("tasks") if isinstance(payload, dict) else None
        if status != 200 or not isinstance(tasks, list):
            raise ResidentError(f"agents HTTP {status}")
        return [t for t in tasks if isinstance(t, dict)]

    async def task_action(self, body):
        status, payload = await self._json("POST", "/v1/tasks", body, total=120)
        return status, payload if isinstance(payload, dict) else {}


def commitment_actions(task_id):
    return (f"`@澪 開始 {task_id}` で外部エージェントに委譲"
            f"（`@澪 開始 {task_id} devin` のように実行者も指定可）／ `@澪 取消 {task_id}`")


def attention_text(task, user):
    """A notice for a task waiting on the operator."""
    task_id = task.get("id", "?")
    detail = task.get("detail") if isinstance(task.get("detail"), dict) else {}
    if task.get("kind") == "commitment":
        return (f"<@{user}> 判断が必要です：澪が引き受けた作業がタスクになっていません（{task_id}）\n"
                f"依頼：{clip(detail.get('message'), 300)}\n"
                f"澪の返事：{clip(detail.get('response'), 300)}\n"
                f"→ {commitment_actions(task_id)}")
    if task.get("kind") == "approval":
        return (f"<@{user}> 承認待ちです（{task_id}）：{clip(task.get('title'), 200)}\n"
                "→ デスクトップの「承認」画面で判断してください。")
    return f"<@{user}> 判断待ちのタスクがあります（{task_id}）：{clip(task.get('title'), 200)}"


VERDICTS = {"accept": "採用", "reject": "不採用"}


def finished_text(task, user):
    """A notice for a delegated task that has ended."""
    detail = task.get("detail") if isinstance(task.get("detail"), dict) else {}
    result = task.get("result") if isinstance(task.get("result"), dict) else {}
    state = {"done": "完了", "failed": "失敗", "cancelled": "取り消し"}.get(task.get("status"), "終了")
    executor = detail.get("executor") or task.get("executor")
    who = f" · {executor}" if executor else ""
    request = clip(str(task.get("title") or "").removeprefix("委譲: "), 300)
    lines = [f"<@{user}> 委譲タスクが{state}しました（{task.get('id', '?')}{who}）", f"依頼：{request}"]
    review = detail.get("review")
    if isinstance(review, dict):
        label = VERDICTS.get(review.get("verdict"), "保留")
        note = f" — {clip(review.get('note'), 200)}" if review.get("note") else ""
        lines.append(f"澪の評価：{label}{note}")
    if result.get("summary"):
        lines.append(f"報告：{clip(result['summary'], 700)}")
    if result.get("branch"):
        lines.append(f"変更はブランチ `{clip(result['branch'], 100)}` にあります。取り込むかはデスクトップの「外部エージェント」で判断してください。")
    return "\n".join(lines)[:MESSAGE_LIMIT]


class NoticeState:
    """Which tasks were already announced, kept across restarts when a state
    directory is available (systemd `StateDirectory=`)."""
    KEEP = 500

    def __init__(self, path=None):
        self.path = path
        self.attention = OrderedDict()   # task id -> {"message": id, "kind": kind}
        self.finished = OrderedDict()    # delegated task id -> message id
        self.delegated_since = None
        if path:
            with suppress(OSError, ValueError, TypeError, AttributeError):
                with open(path, encoding="utf-8") as f:
                    saved = json.load(f)
                self.attention.update(saved.get("attention", {}))
                self.finished.update(saved.get("finished", {}))
                self.delegated_since = saved.get("delegated_since")

    def claim(self, task_id, message_id, kind):
        self.attention[task_id] = {"message": message_id, "kind": kind}
        self._trim()

    def task_for_message(self, message_id):
        for task_id, entry in reversed(self.attention.items()):
            if entry.get("message") == message_id:
                return task_id
        return None

    def _trim(self):
        for book in (self.attention, self.finished):
            while len(book) > self.KEEP:
                book.popitem(last=False)

    def save(self):
        if not self.path:
            return
        self._trim()
        tmp = self.path + ".tmp"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump({"attention": self.attention, "finished": self.finished,
                           "delegated_since": self.delegated_since}, f)
            os.replace(tmp, self.path)
        except OSError as exc:
            LOG.warning("notice state not saved (%s)", type(exc).__name__)


class Notifier:
    """Tells the operator, with a mention, when a task needs their decision
    or delegated work has ended. Polls the resident; sends nothing else."""
    EVERY = 30
    # A commitment made in a Discord turn is announced in that turn's reply.
    GRACE = 90
    # How long a finished delegated task waits for the individual's review.
    REVIEW_WAIT = 600

    def __init__(self, config, resident, send, state, clock=time.time):
        self.config = config
        self.resident = resident
        self.send = send
        self.state = state
        self.clock = clock

    async def run(self):
        while True:
            try:
                await self.poll()
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                LOG.warning("notice poll failed (%s)", type(exc).__name__)
            await asyncio.sleep(self.EVERY)

    async def poll(self):
        now = self.clock()
        changed = False
        for task in await self.resident.tasks():
            task_id = task.get("id")
            if (task.get("status") != "awaiting_operator" or not isinstance(task_id, str)
                    or task_id in self.state.attention):
                continue
            if task.get("kind") == "commitment" and now - float(task.get("created_at") or 0) < self.GRACE:
                continue
            message_id = await self.send(attention_text(task, self.config.notify_user))
            if message_id is not None:
                self.state.claim(task_id, message_id, task.get("kind"))
                changed = True
        try:
            delegated = await self.resident.delegated()
        except (ResidentError, aiohttp.ClientError, asyncio.TimeoutError):
            delegated = None
        if delegated is not None:
            if self.state.delegated_since is None:
                # Start from now: never announce what ended before notices existed.
                self.state.delegated_since = now
                changed = True
            for task in delegated:
                task_id = task.get("id")
                finished = float(task.get("finished_at") or 0)
                if (task.get("status") not in ("done", "failed", "cancelled")
                        or not isinstance(task_id, str) or task_id in self.state.finished
                        or finished < self.state.delegated_since):
                    continue
                detail = task.get("detail") if isinstance(task.get("detail"), dict) else {}
                if (task.get("status") == "done" and detail.get("review") is None
                        and now - finished < self.REVIEW_WAIT):
                    continue
                message_id = await self.send(finished_text(task, self.config.notify_user))
                if message_id is not None:
                    self.state.finished[task_id] = message_id
                    changed = True
        if changed:
            self.state.save()


class DialogueBridge:
    # Matches the resident's default dialogue.max_concurrent.
    WORKERS = 2

    def __init__(self, config, resident, queue_size=16, workers=WORKERS, notices=None):
        self.config = config
        self.resident = resident
        self.queue = asyncio.Queue(maxsize=queue_size)
        self.seen = OrderedDict()
        self.workers = workers
        self.notices = notices if notices is not None else NoticeState()
        # One lock per subject keeps each conversation in order while other
        # subjects proceed. Bounded by the allowlists (users x channels).
        self.subject_locks = {}

    async def reply(self, message, text, mention=None, **kwargs):
        """The sent message (True when the library returned none), or None on failure."""
        try:
            sent = await message.reply(text, mention_author=False,
                                       allowed_mentions=operator_only(mention),
                                       suppress_embeds=True, **kwargs)
            return True if sent is None else sent
        except (discord.HTTPException, aiohttp.ClientError, asyncio.TimeoutError) as exc:
            LOG.warning("Discord reply failed (%s)", type(exc).__name__)
            return None

    def remember(self, message):
        self.seen[message.id] = None
        if len(self.seen) > 4096:
            self.seen.popitem(last=False)

    async def handle(self, message, bot_id):
        if not self.config.allows(message):
            return
        mention = rf"<@!?{bot_id}>"
        if not re.search(mention, message.content) or message.id in self.seen:
            return
        text = re.sub(mention, "", message.content).strip()
        if not text or len(text.encode("utf-8")) > 32768:
            await self.reply(message, "メンションに続けて、32KiB以内の文章を送ってください。")
            return
        command = COMMAND_RE.fullmatch(text)
        if command:
            self.remember(message)
            await self.command(message, command)
            return
        # No await between admission and remembering the ID: Gateway replay is deduped.
        if self.queue.full():
            await self.reply(message, "待ち行列がいっぱいです。少し後で新しいメッセージを送ってください。")
            return
        accepted = asyncio.get_running_loop().create_future()
        self.queue.put_nowait((message, text, accepted))
        self.remember(message)
        try:
            ok = await self.reply(message, f"受け付けました。順番に返事します（待ち {self.queue.qsize()} 件）。")
            accepted.set_result(ok is not None)
        finally:
            if not accepted.done():
                accepted.set_result(False)

    async def command(self, message, match):
        """Start or cancel a commitment the operator was told about."""
        verb = match["verb"].lower()
        start = verb in ("開始", "start")
        task_id = match["id"]
        if task_id is None:
            reference = getattr(message, "reference", None)
            referenced = getattr(reference, "message_id", None)
            task_id = self.notices.task_for_message(referenced) if referenced else None
        if task_id is None:
            await self.reply(message, "どのタスクか分かりませんでした。通知に返信するか、`@澪 開始 t…` のようにIDを付けてください。")
            return
        entry = self.notices.attention.get(task_id)
        if entry is not None and entry.get("kind") not in (None, "commitment"):
            await self.reply(message, f"{task_id} は開始・取消できる引受ではありません。")
            return
        if start:
            body = {"action": "start", "id": task_id}
            if match["executor"]:
                body["executor"] = match["executor"].lower()
        else:
            try:
                tasks = await self.resident.tasks()
            except (ResidentError, aiohttp.ClientError, asyncio.TimeoutError):
                tasks = None
            task = next((t for t in tasks or [] if t.get("id") == task_id), None)
            if task is None or task.get("kind") != "commitment" or task.get("status") != "awaiting_operator":
                await self.reply(message, f"{task_id} は未着手の引受として見つかりませんでした（既に処理済みかもしれません）。")
                return
            body = {"action": "update", "id": task_id, "status": "cancelled",
                    "note": "操作者が取り消し（Discord）"}
        try:
            status, payload = await self.resident.task_action(body)
        except (ResidentError, aiohttp.ClientError, asyncio.TimeoutError) as exc:
            LOG.warning("task action failed (%s)", type(exc).__name__)
            await self.reply(message, "Pi側に届きませんでした。自動再送はしていません。")
            return
        if status != 200:
            reason = {404: "見つからないか、外部エージェントの実行ノードに届きません",
                      409: "既に処理済みです"}.get(status, f"HTTP {status}")
            await self.reply(message, f"{task_id} を{'開始' if start else '取り消し'}できませんでした（{reason}）。詳細はデスクトップのタスク画面で確認してください。")
            return
        if start:
            delegated = payload.get("delegated") if isinstance(payload.get("delegated"), dict) else {}
            await self.reply(message, f"委譲しました → {delegated.get('task_id', '?')}"
                                      f"（{delegated.get('executor', '?')} @ {payload.get('node') or '?'}）。"
                                      "終わったらここでお知らせします。")
        else:
            await self.reply(message, f"{task_id} を取り消しました。")

    async def run(self):
        """A fixed pool of consumers; never start a task per dialogue."""
        await asyncio.gather(*(self.consume() for _ in range(self.workers)))

    async def consume(self):
        while True:
            message, text, accepted = await self.queue.get()
            try:
                subject = self.config.subject(message)
                # No await between taking the item and queueing on its lock:
                # asyncio locks are FIFO, so a subject's turns keep their order.
                async with self.subject_locks.setdefault(subject, asyncio.Lock()):
                    await self.answer(message, text, accepted, subject)
            finally:
                self.queue.task_done()

    def background_note(self, background):
        """What the reply left running, as a line under it, and the
        commitment it announces (if any)."""
        if not isinstance(background, dict):
            return "", None
        if background.get("state") == "started":
            tasks = [t for t in background.get("tasks") or [] if isinstance(t, dict)]
            labels = "、".join(f"{t.get('id', '?')}" + (f"（{t['executor']}）" if t.get("executor") else "")
                              for t in tasks)
            return f"\n\n▶ 裏で実行中：{labels}。終わったらお知らせします。", None
        task_id = background.get("task_id")
        if background.get("state") == "unstarted" and isinstance(task_id, str) and self.config.notifies:
            return (f"\n\n⚠ <@{self.config.notify_user}> 引き受けましたが、タスクはまだ作成されていません（{task_id}）。\n"
                    f"→ {commitment_actions(task_id)}"), task_id
        return "", None

    async def answer(self, message, text, accepted, subject):
        try:
            if not await accepted:
                return
            try:
                reply = await self.resident.talk(text, subject)
            except (aiohttp.ClientError, asyncio.TimeoutError, ResidentError) as exc:
                LOG.warning("resident request failed (%s)", type(exc).__name__)
                await self.reply(message, "澪の応答を受け取れませんでした。Pi側で処理が続いている可能性があります。自動再送はしていません。")
                return
            response = reply["response"]
            note, commitment = self.background_note(reply.get("background"))
            mention = self.config.notify_user if commitment else None
            if len((response + note).encode("utf-16-le")) // 2 <= MESSAGE_LIMIT:
                sent = await self.reply(message, response + note, mention=mention)
            else:
                file = discord.File(io.BytesIO(response.encode("utf-8")), filename="mio-response.txt")
                try:
                    sent = await self.reply(message, "長い応答をファイルにまとめました。" + note,
                                            mention=mention, file=file)
                finally:
                    file.close()
            if commitment and getattr(sent, "id", None) is not None:
                self.notices.claim(commitment, sent.id, "commitment")
                self.notices.save()
        except Exception as exc:
            # A failed item must not kill the worker or leak private bodies.
            LOG.error("queue item failed (%s)", type(exc).__name__)
            await self.reply(message, "この呼び出しの処理に失敗しました。自動再送はしていません。")


class MioClient(discord.Client):
    def __init__(self, config, state_path=None):
        # Explicit @mentions are readable without privileged MESSAGE_CONTENT.
        intents = discord.Intents.none()
        intents.guilds = True
        intents.guild_messages = True
        super().__init__(intents=intents,
                         allowed_mentions=discord.AllowedMentions.none())
        self.config = config
        self.state_path = state_path
        self.session = None
        self.bridge = None
        self.worker = None
        self.notifier = None

    async def setup_hook(self):
        self.session = aiohttp.ClientSession(trust_env=False)
        resident = Resident(self.session)
        notices = NoticeState(self.state_path)
        self.bridge = DialogueBridge(self.config, resident, notices=notices)
        self.worker = asyncio.create_task(self.bridge.run(), name="discord-dialogue-queue")
        if self.config.notifies:
            notifier = Notifier(self.config, resident, self.notify, notices)
            self.notifier = asyncio.create_task(self.start_notices(notifier), name="discord-notices")

    async def start_notices(self, notifier):
        await self.wait_until_ready()
        await notifier.run()

    async def notify(self, text):
        """Post `text` in the notice channel, mentioning only the operator."""
        try:
            channel = self.get_channel(self.config.notify_channel)
            if channel is None:
                channel = await self.fetch_channel(self.config.notify_channel)
            sent = await channel.send(text[:MESSAGE_LIMIT],
                                      allowed_mentions=operator_only(self.config.notify_user),
                                      suppress_embeds=True)
            return sent.id
        except (discord.HTTPException, aiohttp.ClientError, asyncio.TimeoutError) as exc:
            LOG.warning("Discord notice failed (%s)", type(exc).__name__)
            return None

    async def on_message(self, message):
        if self.user is not None:
            await self.bridge.handle(message, self.user.id)

    async def on_ready(self):
        LOG.info("Discord mention gateway ready (notices %s)",
                 "on" if self.config.notifies else "off")

    async def on_error(self, event_method, *args, **kwargs):
        LOG.error("Discord event failed (%s)", event_method)

    async def close(self):
        for task in (self.worker, self.notifier):
            if task:
                task.cancel()
                with suppress(asyncio.CancelledError):
                    await task
        if self.session:
            await self.session.close()
        await super().close()


def main():
    logging.basicConfig(level=logging.INFO)
    # discord.py's default event exception logs can include sensitive URLs.
    logging.getLogger("discord").setLevel(logging.CRITICAL)
    try:
        config = Config.from_env()
    except ValueError as exc:
        raise SystemExit(str(exc)) from None
    state_dir = os.environ.get("STATE_DIRECTORY", "").split(":")[0]
    state_path = os.path.join(state_dir, "notices.json") if state_dir else None
    try:
        MioClient(config, state_path).run(config.token, log_handler=None)
    except Exception as exc:
        LOG.error("Discord client stopped (%s)", type(exc).__name__)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
