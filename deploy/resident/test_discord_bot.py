import asyncio
from contextlib import suppress
import os
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

import aiohttp
from aiohttp import web
import discord

from discord_bot import Config, DialogueBridge, MioClient, Resident, ResidentError, subject_for

CONFIG = Config('fake-token', 10, frozenset({20}), frozenset({30}))


def message(id=1, guild=10, channel=30, user=20, bot=False, text='<@99> こんにちは', webhook=None):
    return SimpleNamespace(id=id, guild=None if guild is None else SimpleNamespace(id=guild),
                           channel=SimpleNamespace(id=channel),
                           author=SimpleNamespace(id=user, bot=bot), content=text,
                           webhook_id=webhook, reply=AsyncMock())


class BridgeTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.backend = SimpleNamespace(talk=AsyncMock(return_value='澪です'))
        self.bridge = DialogueBridge(CONFIG, self.backend)
        self.worker = asyncio.create_task(self.bridge.run())

    async def asyncTearDown(self):
        self.worker.cancel()
        with suppress(asyncio.CancelledError):
            await self.worker

    async def drain(self):
        await asyncio.wait_for(self.bridge.queue.join(), timeout=2)

    async def test_unauthorized_non_mentions_and_bots_are_ignored(self):
        for event in [message(guild=None), message(guild=11), message(channel=31),
                      message(user=21), message(bot=True), message(webhook=22),
                      message(text='hello'), message(text='@everyone'), message(text='<@98> hi')]:
            await self.bridge.handle(event, 99)
            event.reply.assert_not_awaited()
        await self.drain()
        self.backend.talk.assert_not_awaited()

    async def test_ack_then_reply_and_mentions_disabled(self):
        event = message(text='<@!99> こんにちは @everyone')
        await self.bridge.handle(event, 99)
        await self.drain()
        self.backend.talk.assert_awaited_once_with('こんにちは @everyone', subject_for(event))
        self.assertIn('受け付け', event.reply.call_args_list[0].args[0])
        self.assertEqual(event.reply.call_args_list[1].args, ('澪です',))
        for call in event.reply.call_args_list:
            self.assertFalse(call.kwargs['mention_author'])
            self.assertEqual(call.kwargs['allowed_mentions'].to_dict(), {'parse': []})

    async def test_same_subject_is_fifo_and_one_at_a_time(self):
        started, finish = asyncio.Event(), asyncio.Event()
        active, max_active, order = 0, 0, []
        async def talk(text, subject):
            nonlocal active, max_active
            active += 1
            max_active = max(max_active, active)
            order.append(text)
            if text == 'first':
                started.set()
                await finish.wait()
            await asyncio.sleep(0)
            active -= 1
            return text
        self.backend.talk.side_effect = talk
        first = message(text='<@99> first')
        await self.bridge.handle(first, 99)
        await started.wait()
        for i, word in enumerate(['second', 'third'], 2):
            await self.bridge.handle(message(id=i, text='<@99> ' + word), 99)
        self.assertEqual(order, ['first'])
        self.assertEqual(self.bridge.queue.qsize(), 2)
        finish.set()
        await self.drain()
        self.assertEqual(order, ['first', 'second', 'third'])
        self.assertEqual(max_active, 1)

    async def test_different_subjects_run_together_up_to_the_pool(self):
        config = Config('fake-token', 10, frozenset({20, 21, 22}), frozenset({30}))
        bridge = DialogueBridge(config, self.backend)
        worker = asyncio.create_task(bridge.run())
        both, finish = asyncio.Event(), asyncio.Event()
        active, max_active = 0, 0
        async def talk(text, subject):
            nonlocal active, max_active
            active += 1
            max_active = max(max_active, active)
            if active == 2:
                both.set()
            await finish.wait()
            active -= 1
            return text
        self.backend.talk.side_effect = talk
        try:
            for i, user in enumerate([20, 21, 22], 1):
                await bridge.handle(message(id=i, user=user, text=f'<@99> m{i}'), 99)
            await asyncio.wait_for(both.wait(), timeout=2)
            await asyncio.sleep(0.05)
            self.assertEqual(max_active, 2)
            finish.set()
            await asyncio.wait_for(bridge.queue.join(), timeout=2)
            self.assertEqual(self.backend.talk.await_count, 3)
        finally:
            worker.cancel()
            with suppress(asyncio.CancelledError):
                await worker

    async def test_duplicate_gateway_message_not_processed_twice(self):
        event = message()
        await asyncio.gather(self.bridge.handle(event, 99), self.bridge.handle(event, 99))
        await self.drain()
        await self.bridge.handle(event, 99)
        self.backend.talk.assert_awaited_once()
        self.assertEqual(event.reply.await_count, 2)

    async def test_queue_overflow_is_not_silently_accepted(self):
        bridge = DialogueBridge(CONFIG, self.backend, queue_size=1)
        await bridge.handle(message(), 99)
        overflow = message(id=2)
        await bridge.handle(overflow, 99)
        self.assertIn('いっぱい', overflow.reply.call_args.args[0])
        self.assertEqual(bridge.queue.qsize(), 1)
        self.assertNotIn(2, bridge.seen)

    async def test_failed_item_does_not_block_next_or_retry(self):
        self.backend.talk.side_effect = [ResidentError('private-body'), asyncio.TimeoutError(), 'done']
        events = [message(id=i) for i in range(3)]
        for event in events:
            await self.bridge.handle(event, 99)
        await self.drain()
        self.assertEqual(self.backend.talk.await_count, 3)
        self.assertIn('自動再送はしていません', events[0].reply.call_args.args[0])
        self.assertNotIn('private-body', str(events[0].reply.call_args))
        self.assertEqual(events[2].reply.call_args.args, ('done',))

    async def test_long_reply_is_complete_attachment(self):
        text = '😀' * 1800 + '\n日本語'
        self.backend.talk.return_value = text
        files = []
        async def reply(*args, **kwargs):
            if 'file' in kwargs:
                files.append(kwargs['file'].fp.read().decode())
        event = message()
        event.reply.side_effect = reply
        await self.bridge.handle(event, 99)
        await self.drain()
        self.assertEqual(files, [text])

    async def test_invalid_input_never_reaches_resident(self):
        for text in ['<@99>  ', '<@99>' + 'あ' * 11000]:
            await self.bridge.handle(message(text=text), 99)
        await self.drain()
        self.backend.talk.assert_not_awaited()

    async def test_ack_failure_skips_generation_and_worker_survives(self):
        event = message()
        event.reply.side_effect = discord.Forbidden(SimpleNamespace(status=403, reason='Forbidden'), 'private')
        await self.bridge.handle(event, 99)
        await self.drain()
        self.backend.talk.assert_not_awaited()
        await self.bridge.handle(message(id=2), 99)
        await self.drain()
        self.backend.talk.assert_awaited_once()

    async def test_response_send_failure_does_not_regenerate(self):
        event = message()
        event.reply.side_effect = [None, discord.NotFound(SimpleNamespace(status=404, reason='Gone'), 'deleted')]
        await self.bridge.handle(event, 99)
        await self.drain()
        self.backend.talk.assert_awaited_once()
        self.assertFalse(self.worker.done())

    async def test_client_worker_lifecycle_and_intents(self):
        client = MioClient(CONFIG)
        try:
            await client.setup_hook()
            self.assertTrue(client.intents.guild_messages)
            self.assertFalse(client.intents.message_content)
            self.assertFalse(client.intents.members)
            self.assertFalse(client.worker.done())
        finally:
            await client.close()
        self.assertTrue(client.worker.done())
        self.assertTrue(client.session.closed)


class ConfigTests(unittest.TestCase):
    def test_missing_and_bad_allowlists_fail_closed(self):
        valid = {'DISCORD_BOT_TOKEN': 'fake', 'DISCORD_GUILD_ID': '10',
                 'DISCORD_ALLOWED_USER_IDS': '20', 'DISCORD_ALLOWED_CHANNEL_IDS': '30'}
        with patch.dict(os.environ, valid, clear=True):
            self.assertEqual(Config.from_env().channels, frozenset({30}))
            self.assertNotIn('fake', repr(Config.from_env()))
        for key in valid:
            for value in ['', '0', 'bad', '18446744073709551616'] if key != 'DISCORD_BOT_TOKEN' else ['']:
                with patch.dict(os.environ, {**valid, key: value}, clear=True):
                    with self.assertRaises(ValueError):
                        Config.from_env()

    def test_operator_subject_is_shared_with_other_surfaces(self):
        config = Config('t', 10, frozenset({20}), frozenset({30}), 'eightman')
        self.assertEqual(config.subject(message()), 'eightman')
        self.assertEqual(CONFIG.subject(message()), subject_for(message()))

    def test_operator_subject_validation(self):
        base = {'DISCORD_BOT_TOKEN': 't', 'DISCORD_GUILD_ID': '10',
                'DISCORD_ALLOWED_USER_IDS': '20', 'DISCORD_ALLOWED_CHANNEL_IDS': '30'}
        with patch.dict(os.environ, {**base, 'DISCORD_OPERATOR_SUBJECT': 'eightman'}, clear=True):
            self.assertEqual(Config.from_env().operator_subject, 'eightman')
        with patch.dict(os.environ, {**base, 'DISCORD_OPERATOR_SUBJECT': ''}, clear=True):
            self.assertIsNone(Config.from_env().operator_subject)
        for bad in ['--dir', 'a b', 'a@b', 'x' * 65]:
            with patch.dict(os.environ, {**base, 'DISCORD_OPERATOR_SUBJECT': bad}, clear=True):
                self.assertRaises(ValueError, Config.from_env)
        with patch.dict(os.environ, {**base, 'DISCORD_ALLOWED_USER_IDS': '20,21',
                                     'DISCORD_OPERATOR_SUBJECT': 'eightman'}, clear=True):
            self.assertRaises(ValueError, Config.from_env)

    def test_subject_is_stable_bounded_and_isolated(self):
        subjects = {subject_for(i) for i in [message(), message(guild=11),
                    message(channel=31), message(user=21)]}
        self.assertEqual(len(subjects), 4)
        self.assertEqual(subject_for(message()), subject_for(message()))
        for subject in subjects:
            self.assertLessEqual(len(subject), 64)
            self.assertRegex(subject, r'^[A-Za-z0-9][A-Za-z0-9_.-]*$')


class ResidentTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.requests = []
        self.status = 200
        self.body = {'response': '澪です', 'stderr_tail': 'do-not-forward'}
        async def talk(request):
            self.requests.append(await request.json())
            return web.json_response(self.body, status=self.status)
        app = web.Application()
        app.router.add_post('/v1/kamimusuhi/talk', talk)
        self.runner = web.AppRunner(app)
        await self.runner.setup()
        server = web.TCPSite(self.runner, '127.0.0.1', 0)
        await server.start()
        port = self.runner.addresses[0][1]
        self.session = aiohttp.ClientSession()
        self.resident = Resident(self.session, f'http://127.0.0.1:{port}')

    async def asyncTearDown(self):
        await self.session.close()
        await self.runner.cleanup()

    async def test_real_http_contract(self):
        self.assertEqual(await self.resident.talk('こんにちは', 'discord-test'), '澪です')
        self.assertEqual(self.requests, [{'message': 'こんにちは', 'subject': 'discord-test'}])

    async def test_bad_status_and_schema(self):
        for status, body in [(503, {'error': 'private'}), (200, []),
                             (200, {'response': 2}), (200, {'response': ' '})]:
            self.status, self.body = status, body
            with self.assertRaises(ResidentError):
                await self.resident.talk('test', 'discord-test')
        self.assertEqual(len(self.requests), 4)

    async def test_large_chunked_response_is_complete_and_bounded(self):
        self.body = {'response': 'あ' * 50000}
        self.assertEqual(await self.resident.talk('test', 'discord-test'), 'あ' * 50000)
        self.body = {'response': 'a' * (1024 * 1024)}
        with self.assertRaises(ResidentError):
            await self.resident.talk('test', 'discord-test')


if __name__ == '__main__':
    unittest.main()
