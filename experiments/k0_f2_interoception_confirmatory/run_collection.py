"""Mac-side private SSH transport with finite, fail-closed owned-process cleanup."""
import argparse
import json
import os
from pathlib import Path
import queue
import shlex
import signal
import subprocess
import threading
import time


class TransportErrors:
    """Keep non-secret exception types; worker failures wake the main loop."""
    def __init__(self):
        self.failed = threading.Event()
        self.rows = []
        self.lock = threading.Lock()

    def record(self, stage, error_type):
        with self.lock:
            self.rows.append(dict(stage=stage, error_type=error_type, timestamp=time.time()))
        self.failed.set()

    def check(self):
        if self.failed.is_set():
            raise RuntimeError('Transport worker failed; inspect transport_state.json error types')


def forward_telemetry(source, receiver, log, stopping, errors):
    try:
        for line in source.stdout:
            log.write(line)
            log.flush()
            receiver.stdin.write(line)
            receiver.stdin.flush()
        if not stopping.is_set():
            errors.record('sensor_forward', 'UnexpectedSensorEOF')
    except Exception as exc:
        if not stopping.is_set():
            errors.record('sensor_forward', type(exc).__name__)


def transfer_network(seconds, block_id, ssh_alias, stopping, block_stop, errors, rows):
    end = time.monotonic() + seconds
    payload = bytes(4 * 1024 * 1024)
    while time.monotonic() < end and not stopping.is_set() and not block_stop.is_set():
        started = time.time()
        entry = dict(timestamp=started, bytes=0, requested_bytes=len(payload), block_id=block_id,
                     source_kind='real', ok=False)
        try:
            result = subprocess.run(['ssh', '-o', 'ConnectTimeout=5', ssh_alias, 'cat > /dev/null'],
                input=payload, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
            entry['returncode'] = result.returncode
            entry['ok'] = result.returncode == 0
            if not entry['ok']:
                entry['error_type'] = 'NetworkTransferExit'
                errors.record('network_transfer', entry['error_type'])
            else:
                entry['bytes'] = len(payload)
        except Exception as exc:
            # Exception text can contain the SSH alias or command; never retain it.
            entry['error_type'] = type(exc).__name__
            errors.record('network_transfer', entry['error_type'])
        finally:
            entry['duration_seconds'] = time.time() - started
            rows.append(entry)
        if not entry['ok']:
            return
        block_stop.wait(.5)


def driver_lines(driver, lines, errors, stopping):
    try:
        for line in driver.stdout:
            lines.put(line)
    except Exception as exc:
        if not stopping.is_set():
            errors.record('driver_output', type(exc).__name__)
    finally:
        lines.put(None)


def remote_stop(args, remote, driver, errors):
    """Also stop after a broken SSH client exits: its remote driver may survive."""
    request = remote([args.remote_python, '-c',
        "from pathlib import Path; import sys; Path(sys.argv[1]).write_text('owned acquisition stop\\n')",
        args.remote_output + '/STOP'])
    try:
        result = subprocess.run(['ssh', '-o', 'ConnectTimeout=5', args.ssh_alias, request],
                                capture_output=True, timeout=10)
        if result.returncode:
            errors.record('remote_stop', 'StopRequestExit')
        if driver is not None and driver.poll() is None:
            try:
                driver.wait(timeout=30)
                return
            except subprocess.TimeoutExpired:
                pass
        # A lost SSH session offers no proof of remote process death. Verify the
        # owned PID and command before signalling; no command line is exported.
        force = remote([args.remote_python, '-c',
            "import json,os,signal,sys; from pathlib import Path; f=Path(sys.argv[1]); "
            "d=json.loads(f.read_text()) if f.exists() else {}; p=d.get('pid'); "
            "c=Path('/proc/'+str(p)+'/cmdline'); "
            "alive=c.exists(); v=c.read_bytes() if alive else b''; "
            "assert not alive or (d.get('owned_by')=='k0-f2-acquire' and "
            "(b'experiments.k0_f2_interoception_confirmatory.acquire' in v or "
            "b'experiments.k0_f2_interoception_confirmatory.live' in v)); "
            "os.kill(p,signal.SIGTERM) if alive else None",
            args.remote_output + '/driver_identity.json'])
        result = subprocess.run(['ssh', '-o', 'ConnectTimeout=5', args.ssh_alias, force],
                                capture_output=True, timeout=10)
        if result.returncode:
            errors.record('remote_stop', 'OwnedSignalExit')
        if driver is not None and driver.poll() is None:
            driver.wait(timeout=15)
    except Exception as exc:
        errors.record('remote_stop', type(exc).__name__)


def finish_process(proc, errors):
    try:
        if proc.poll() is None:
            proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
    except Exception as exc:
        errors.record('local_process_stop', type(exc).__name__)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ('ssh-alias', 'peer-host', 'remote-root', 'remote-python', 'remote-output',
                'session-id', 'study-root', 'protocol-lock'):
        parser.add_argument('--' + key, required=True)
    parser.add_argument('--sensor-binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--blocks', type=int, default=16)
    parser.add_argument('--block-seconds', type=float, default=45)
    parser.add_argument('--phase', choices=['record', 'live'], default='record')
    parser.add_argument('--training-artifacts')
    parser.add_argument('--dataset')
    args = parser.parse_args()
    module = 'experiments.k0_f2_interoception_confirmatory.' + ('live' if args.phase == 'live' else 'acquire')
    extra = ['--session-id', args.session_id, '--study-root', args.study_root, '--protocol-lock', args.protocol_lock]
    if args.phase == 'live':
        if not args.training_artifacts or not args.dataset:
            raise ValueError('live requires frozen models and test body donor records')
        extra += ['--training-artifacts', args.training_artifacts, '--dataset', args.dataset]
    args.output.mkdir(parents=True, exist_ok=True)
    if (args.output / 'transport_state.json').exists():
        raise FileExistsError('Use fresh transport output')
    def remote(words):
        return 'cd ' + shlex.quote(args.remote_root) + ' && ' + ' '.join(shlex.quote(str(w)) for w in words)
    def stop(*_):
        raise KeyboardInterrupt('finite collection stopped')
    signal.signal(signal.SIGTERM, stop)
    processes, threads, network_rows = [], [], []
    errors, stopping, active_network_stop = TransportErrors(), threading.Event(), threading.Event()
    source = receiver = driver = mac_log = remote_log = None
    success = False
    try:
        activity = subprocess.Popen(['/usr/bin/caffeinate', '-i', '-w', str(os.getpid())],
                                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        processes.append(activity)
        receiver = subprocess.Popen(['ssh', '-o', 'ConnectTimeout=5', args.ssh_alias,
            remote([args.remote_python, '-m', 'experiments.k0_f2_interoception_confirmatory.acquire',
                    'receive', '--output', args.remote_output + '/raw_mac_telemetry.jsonl'])],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        processes.append(receiver)
        source = subprocess.Popen([str(args.sensor_binary), '--duration', str(args.blocks * args.block_seconds + 450),
            '--interval', '1', '--peer-host', args.peer_host], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        processes.append(source)
        mac_log = (args.output / 'raw_mac_source.jsonl').open('wb')
        thread = threading.Thread(target=forward_telemetry, args=(source, receiver, mac_log, stopping, errors), daemon=True)
        thread.start(); threads.append(thread)
        driver = subprocess.Popen(['ssh', '-o', 'ConnectTimeout=5', args.ssh_alias,
            remote([args.remote_python, '-m', module, 'collect', '--output', args.remote_output,
                    '--blocks', args.blocks, '--block-seconds', args.block_seconds] + extra)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        processes.append(driver)
        lines = queue.Queue()
        thread = threading.Thread(target=driver_lines, args=(driver, lines, errors, stopping), daemon=True)
        thread.start(); threads.append(thread)
        remote_log = (args.output / 'collection.log').open('w')
        while True:
            errors.check()
            if receiver.poll() is not None:
                raise RuntimeError('Telemetry receiver exited before collection completion')
            try:
                line = lines.get(timeout=.2)
            except queue.Empty:
                continue
            if line is None:
                break
            remote_log.write(line); remote_log.flush(); print(line.strip(), flush=True)
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if event.get('label') == 'network_transfer':
                active_network_stop.set()
                active_network_stop = threading.Event()
                thread = threading.Thread(target=transfer_network,
                    args=(event['seconds'], event['block_id'], args.ssh_alias, stopping, active_network_stop, errors, network_rows), daemon=True)
                thread.start(); threads.append(thread)
            if event.get('event') == 'block_end':
                active_network_stop.set()
        code = driver.wait(timeout=5)
        errors.check()
        if code != 0:
            raise RuntimeError('Remote collection failed; see local log')
        success = True
    except BaseException as exc:
        errors.record('main', type(exc).__name__)
        raise
    finally:
        stopping.set(); active_network_stop.set()
        if not success and driver is not None:
            remote_stop(args, remote, driver, errors)
        if source is not None:
            finish_process(source, errors)
        for thread in threads:
            thread.join(timeout=12)
            if thread.is_alive():
                errors.record('thread_stop', 'ThreadStillAlive')
        if receiver is not None and receiver.stdin:
            try:
                receiver.stdin.close()
            except (BrokenPipeError, OSError):
                pass
        for proc in reversed(processes):
            finish_process(proc, errors)
        if mac_log:
            mac_log.close()
        if remote_log:
            remote_log.close()
        (args.output / 'network_workloads.json').write_text(json.dumps(network_rows, indent=2) + '\n')
        remote_state = None
        try:
            verification = subprocess.run(['ssh', '-o', 'ConnectTimeout=5', args.ssh_alias,
                remote([args.remote_python, '-c', 'from pathlib import Path; import sys; print(Path(sys.argv[1]).read_text())',
                        args.remote_output + '/collection_runtime_state.json'])], capture_output=True, text=True, timeout=10)
            if verification.returncode != 0:
                raise RuntimeError('RemoteCleanupStateUnavailable')
            remote_state = json.loads(verification.stdout)
            if not remote_state.get('owned_sensor_stopped') or not remote_state.get('owned_background_stopped'):
                raise RuntimeError('RemoteOwnedProcessesNotStopped')
            if success and not remote_state.get('collection_complete'):
                raise RuntimeError('RemoteCollectionIncomplete')
        except Exception as exc:
            errors.record('remote_verification', type(exc).__name__)
        success = success and not errors.failed.is_set()
        (args.output / 'transport_state.json').write_text(json.dumps(dict(timestamp=time.time(), success=success,
            cleanup_errors=errors.rows, transport_errors=errors.rows, remote_cleanup_state=remote_state,
            owned_processes=[dict(pid=proc.pid, returncode=proc.poll()) for proc in processes],
            sensor_source_retained=True, connection_values_excluded=True), indent=2) + '\n')
        # A zero remote exit cannot conceal a network worker or cleanup failure.
        if not success and not any(r['stage'] == 'main' for r in errors.rows):
            raise RuntimeError('Transport or cleanup verification failed')


if __name__ == '__main__':
    main()
