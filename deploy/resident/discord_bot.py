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

import aiohttp
import discord

LOG = logging.getLogger("kamimusuhi.discord")
RESIDENT_URL = "http://127.0.0.1:7860"
# Accepted by both the resident API and `kamimusuhi-runtime talk --subject`.
SUBJECT_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}")


def snowflakes(value: str) -> frozenset[int]:
    parts = value.split(",")
    if not all(p.strip().isascii() and p.strip().isdigit() for p in parts):
        raise ValueError("Discord IDs must be a nonempty comma-separated list")
    ids = frozenset(int(p) for p in parts)
    if not all(0 < n < 2**64 for n in ids):
        raise ValueError("Discord IDs must be positive 64-bit integers")
    return ids


@dataclass(frozen=True)
class Config:
    token: str = field(repr=False)
    guild_id: int
    users: frozenset[int]
    channels: frozenset[int]
    # The operator's own subject (the one the desktop uses, e.g. $USER), so
    # Discord turns share relationship memory and recall with other surfaces.
    operator_subject: str | None = None

    @classmethod
    def from_env(cls):
        token = os.environ.get("DISCORD_BOT_TOKEN", "").strip()
        if not token:
            raise ValueError("DISCORD_BOT_TOKEN is required")
        guilds = snowflakes(os.environ.get("DISCORD_GUILD_ID", ""))
        if len(guilds) != 1:
            raise ValueError("DISCORD_GUILD_ID must contain one ID")
        users = snowflakes(os.environ.get("DISCORD_ALLOWED_USER_IDS", ""))
        operator = os.environ.get("DISCORD_OPERATOR_SUBJECT", "").strip() or None
        if operator is not None:
            if not SUBJECT_RE.fullmatch(operator):
                raise ValueError("DISCORD_OPERATOR_SUBJECT must be 1-64 chars of [A-Za-z0-9._-]")
            if len(users) != 1:
                # Sharing one subject between people would merge their memories.
                raise ValueError("DISCORD_OPERATOR_SUBJECT requires exactly one allowed user")
        return cls(token, next(iter(guilds)), users,
                   snowflakes(os.environ.get("DISCORD_ALLOWED_CHANNEL_IDS", "")),
                   operator)

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


class ResidentError(Exception):
    pass


class Resident:
    def __init__(self, session, url=RESIDENT_URL):
        self.session = session
        self.url = url

    async def talk(self, message, subject):
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
            return text


class DialogueBridge:
    # Matches the resident's default dialogue.max_concurrent.
    WORKERS = 2

    def __init__(self, config, resident, queue_size=16, workers=WORKERS):
        self.config = config
        self.resident = resident
        self.queue = asyncio.Queue(maxsize=queue_size)
        self.seen = OrderedDict()
        self.workers = workers
        # One lock per subject keeps each conversation in order while other
        # subjects proceed. Bounded by the allowlists (users x channels).
        self.subject_locks = {}

    async def reply(self, message, text, **kwargs):
        try:
            await message.reply(text, mention_author=False,
                                allowed_mentions=discord.AllowedMentions.none(),
                                suppress_embeds=True, **kwargs)
            return True
        except (discord.HTTPException, aiohttp.ClientError, asyncio.TimeoutError) as exc:
            LOG.warning("Discord reply failed (%s)", type(exc).__name__)
            return False

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
        # No await between admission and remembering the ID: Gateway replay is deduped.
        if self.queue.full():
            await self.reply(message, "待ち行列がいっぱいです。少し後で新しいメッセージを送ってください。")
            return
        accepted = asyncio.get_running_loop().create_future()
        self.queue.put_nowait((message, text, accepted))
        self.seen[message.id] = None
        if len(self.seen) > 4096:
            self.seen.popitem(last=False)
        try:
            ok = await self.reply(message, f"受け付けました。順番に返事します（待ち {self.queue.qsize()} 件）。")
            accepted.set_result(ok)
        finally:
            if not accepted.done():
                accepted.set_result(False)

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

    async def answer(self, message, text, accepted, subject):
        try:
            if not await accepted:
                return
            try:
                response = await self.resident.talk(text, subject)
            except (aiohttp.ClientError, asyncio.TimeoutError, ResidentError) as exc:
                LOG.warning("resident request failed (%s)", type(exc).__name__)
                await self.reply(message, "澪の応答を受け取れませんでした。Pi側で処理が続いている可能性があります。自動再送はしていません。")
                return
            if len(response.encode("utf-16-le")) // 2 <= 1900:
                await self.reply(message, response)
            else:
                file = discord.File(io.BytesIO(response.encode("utf-8")), filename="mio-response.txt")
                try:
                    await self.reply(message, "長い応答をファイルにまとめました。", file=file)
                finally:
                    file.close()
        except Exception as exc:
            # A failed item must not kill the worker or leak private bodies.
            LOG.error("queue item failed (%s)", type(exc).__name__)
            await self.reply(message, "この呼び出しの処理に失敗しました。自動再送はしていません。")


class MioClient(discord.Client):
    def __init__(self, config):
        # Explicit @mentions are readable without privileged MESSAGE_CONTENT.
        intents = discord.Intents.none()
        intents.guilds = True
        intents.guild_messages = True
        super().__init__(intents=intents,
                         allowed_mentions=discord.AllowedMentions.none())
        self.config = config
        self.session = None
        self.bridge = None
        self.worker = None

    async def setup_hook(self):
        self.session = aiohttp.ClientSession(trust_env=False)
        self.bridge = DialogueBridge(self.config, Resident(self.session))
        self.worker = asyncio.create_task(self.bridge.run(), name="discord-dialogue-queue")

    async def on_message(self, message):
        if self.user is not None:
            await self.bridge.handle(message, self.user.id)

    async def on_ready(self):
        LOG.info("Discord mention gateway ready")

    async def on_error(self, event_method, *args, **kwargs):
        LOG.error("Discord event failed (%s)", event_method)

    async def close(self):
        if self.worker:
            self.worker.cancel()
            with suppress(asyncio.CancelledError):
                await self.worker
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
    try:
        MioClient(config).run(config.token, log_handler=None)
    except Exception as exc:
        LOG.error("Discord client stopped (%s)", type(exc).__name__)
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
