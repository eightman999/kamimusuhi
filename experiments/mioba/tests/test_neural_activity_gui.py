"""Execute the shipped GUI JavaScript with controlled fetch/DOM/timers.

This checks asynchronous identity and freshness boundaries, not browser
layout or live backend sampling. No coordinator or browser is started.
"""
from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess

import pytest


INDEX = Path(__file__).resolve().parents[1] / "gui" / "static" / "index.html"
NODE = shutil.which("node")

HARNESS = r"""
const fs = require('node:fs');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const html = fs.readFileSync(process.argv[2], 'utf8');
const script = html.match(/<script>([\s\S]*?)<\/script>/)[1];
const timers = new Map(); let nextTimer = 0;
const clock = {now:0};
const elements = new Map();
function element(selector) {
  if (!elements.has(selector)) elements.set(selector, {
    dataset: {}, innerHTML:'', textContent:'', className:'', title:'',
    appended:[], clientWidth:600, clientHeight:300, listeners:{},
    addEventListener(type, fn) { this.listeners[type] = fn; },
    insertAdjacentHTML(position, text) { this.appended.push(text); },
    getBoundingClientRect() { return {left:0,top:0}; },
    classList:{toggle(){}},
    drawing:[],
    getContext() {
      const log = this.drawing;
      return {scale(){},clearRect(){log.push('clear');},fillRect(){log.push({kind:'cell',fill:this.fillStyle});},
        fillText(text){log.push(text);},beginPath(){},moveTo(){},lineTo(){},stroke(){},setLineDash(){},
        ellipse(){},arc(){},quadraticCurveTo(){},closePath(){},fill(){},
        measureText(text){return {width:String(text).length*5};}};
    },
  });
  return elements.get(selector);
}
const context = vm.createContext({
  assert, console, AbortController, DOMException, Date, Intl, clock,
  performance:{now:()=>clock.now},
  setTimeout(fn, ms) { const id = ++nextTimer; timers.set(id,{fn,ms}); return id; },
  clearTimeout(id) { timers.delete(id); },
  fireTimer(ms) {
    const entry = [...timers].find(([,timer])=>timer.ms === ms);
    assert.ok(entry, `no ${ms} ms timer`);
    timers.delete(entry[0]); return entry[1].fn();
  },
  timerCount(ms) { return [...timers.values()].filter(timer=>timer.ms === ms).length; },
  element,
  localStorage:{getItem(){return null;},setItem(){}},
  sessionStorage:{getItem(){return null;},setItem(){}},
  document:{hidden:false,documentElement:{},querySelector:element,querySelectorAll(){return [];}},
  window:{devicePixelRatio:1,addEventListener(){},removeEventListener(){}},
  location:{hash:'#/organism/genome-a'},
  fetch(){throw new Error('unexpected fetch');},
});
vm.runInContext(script,context,{filename:'index.html'});
vm.runInContext(`
const BASE = 1790000000000;
function payload(sequence=1, changes={}, outer={}) {
  const frame = {schema_version:1, experiment_id:'experiment-a', genome_id:'genome-a',
    job_id:'job-a', worker_id:'worker-a', attempt:1, evaluation_pass:0, sequence,
    backend:'torch', phase:'evaluation', replicate_index:0, replicate_seed:9223372036854775807,
    n_neurons:100, n_base:50, neuron_indices:[0,12,50,99], spike_counts:[0,1,3,0],
    window_start_ms:sequence*10, window_end_ms:(sequence+1)*10, received_at_unix_ms:BASE+sequence*100,
    ...changes};
  return {schema_version:1,kind:'LIVE',experiment_id:'experiment-a',genome_id:'genome-a',status:'running',
    observed_at_unix_ms:frame.received_at_unix_ms,age_ms:0,ttl_ms:5000,job_status:'RUNNING',frame,...outer};
}
function deferred() { let resolve,reject; const promise = new Promise((a,b)=>{resolve=a;reject=b;}); return {promise,resolve,reject}; }
async function settle() { for(let i=0;i<12;i++) await Promise.resolve(); }
function response(body) { return {ok:true,json:async()=>body}; }
function detail(id='genome-a') { return {kind:'RECORDED',genome:{genome_id:id,experiment_id:'experiment-a'},
  genome_doc:{},phenotype:{kind:'DERIVED',params:{},ancestry_fraction:0},clades:[],ancestry_chain:{kind:'DERIVED',rows:[]},parents:[],children:[],evaluations:[]}; }
element('#neural-activity').dataset.genomeId = 'genome-a';
`,context);
"""


def run_js(case: str) -> None:
    if NODE is None:
        pytest.skip("Node.js is needed to execute the shipped GUI JavaScript")
    program = HARNESS + "\nvm.runInContext(" + json.dumps(
        "(async()=>{\n" + case + "\n})()"
    ) + ",context).catch(error=>{console.error(error.stack);process.exitCode=1;});\n"
    result = subprocess.run([NODE, "-", str(INDEX)], input=program, text=True,
                            capture_output=True, timeout=15, check=False)
    assert result.returncode == 0, result.stderr


def test_real_zero_spikes_mock_label_and_missing_data_are_distinct():
    run_js(r"""
const state = new NeuralActivityState('genome-a','experiment-a');
renderNeuralActivity(state,0);
assert.equal(state.view(0).frame,null);
assert.equal(state.history.length,0);
assert.equal(element('#neural-status').textContent,t('n','waiting'));
state.accept(payload(1,{backend:'mock',spike_counts:[0,0,0,0]}),0);
renderNeuralActivity(state,0);
assert.equal(state.view(0).live,true);
assert.equal(state.history.length,1);
assert.equal(state.history[0].active,0);
assert.ok(element('#neural-backend').textContent.includes('mock'));
assert.ok(element('#neural-meta').innerHTML.includes('4 / 100'));
assert.ok(element('#neural-meta').innerHTML.includes('代表replicate'));
assert.ok(element('#neural-meta').innerHTML.includes('10.00–20.00 ms'));
assert.ok(element('#neural-meta').innerHTML.includes('JST'));
assert.ok(!element('#neural-meta').innerHTML.includes(String(state.frame.replicate_seed)));
assert.equal(element('#neural-grid').neuralCells.length,4);
state.fail(); renderNeuralActivity(state,100);
assert.equal(state.view(100).live,false);
assert.equal(state.view(100).kind,'RECORDED');
assert.ok(element('#neural-note').textContent.includes('ゼロ発火ではありません'));
assert.ok(!element('#neural-dot').className.includes('is-live'));
assert.equal(state.history.length,1);
assert.equal(state.frame.sequence,1);
const mixed = new NeuralActivityState('genome-a','experiment-a');
mixed.accept(payload(1),0); renderNeuralActivity(mixed,0);
const liveColors = element('#neural-grid').drawing.filter(item=>item.kind==='cell').slice(-4).map(item=>item.fill);
mixed.fail(); renderNeuralActivity(mixed,100);
const recordedColors = element('#neural-grid').drawing.filter(item=>item.kind==='cell').slice(-4).map(item=>item.fill);
assert.equal(JSON.stringify(recordedColors),JSON.stringify(liveColors));
assert.equal(recordedColors[0],'#343d4b');
assert.ok(recordedColors[1].includes('90,169,255'));
assert.ok(recordedColors[2].includes('255,159,67'));
""")


def test_repeated_and_out_of_order_sequences_do_not_refresh_or_extend_history():
    run_js(r"""
const state = new NeuralActivityState('genome-a','experiment-a');
state.accept(payload(2),0);
assert.equal(state.history.length,1);
assert.equal(state.accept(payload(2,{}, {observed_at_unix_ms:BASE+3200,age_ms:3000}),3000),false);
assert.equal(state.history.length,1);
assert.equal(state.accept(payload(2,{}, {observed_at_unix_ms:BASE+6200,age_ms:0}),6000),false);
assert.equal(state.view(6000).live,false,'duplicate response refreshed a cached frame');
assert.equal(state.view(6000).status,'stale');
assert.equal(state.accept(payload(1,{}, {observed_at_unix_ms:BASE+6300,age_ms:0}),6100),false);
assert.equal(state.frame.sequence,2);
assert.equal(state.history.length,1);
assert.equal(state.view(6100).live,false);
assert.equal(state.accept(payload(3,{}, {observed_at_unix_ms:BASE+100}),6200),false);
assert.equal(state.frame.sequence,2,'older envelope displaced the current frame');
""")


def test_job_attempt_pass_and_replicate_changes_reset_the_history():
    run_js(r"""
for (const changed of [{job_id:'job-b'},{attempt:2},{evaluation_pass:1},{replicate_index:1}]) {
  const state = new NeuralActivityState('genome-a','experiment-a');
  state.accept(payload(10),0); state.accept(payload(11),100);
  assert.equal(state.history.length,2);
  state.accept(payload(0,{...changed,received_at_unix_ms:BASE+2000}),200);
  assert.equal(state.history.length,1);
  assert.equal(state.sequence,0);
  assert.equal(state.history[0].sequence,0);
  state.accept(payload(12,{received_at_unix_ms:BASE+1200},{observed_at_unix_ms:BASE+2500}),250);
  assert.equal(state.sequence,0,'older session replaced the new job/pass');
  assert.equal(state.history.length,1);
}
""")


def test_recorded_waiting_and_stale_envelopes_stop_live_history_progression():
    run_js(r"""
const state = new NeuralActivityState('genome-a','experiment-a');
state.accept(payload(1),0);
state.accept(payload(2,{}, {kind:'RECORDED',status:'recorded',job_status:'SUCCEEDED'}),100);
assert.equal(state.view(100).live,false);
assert.equal(state.view(100).status,'recorded');
assert.equal(state.history.length,1);
assert.equal(state.frame.sequence,2);
state.accept(payload(3,{}, {kind:'UNAVAILABLE',status:'waiting',job_status:null,frame:null,age_ms:null}),200);
assert.equal(state.view(200).live,false);
assert.equal(state.frame,null);
assert.equal(state.history.length,0);
assert.equal(state.sequence,-1);
assert.equal(state.sessionKey,null);
assert.equal(state.ageBase,null);
renderNeuralActivity(state,200);
assert.equal(element('#neural-grid').neuralCells.length,0);
assert.equal(element('#neural-status').textContent,t('n','waiting'));
const stale = new NeuralActivityState('genome-a');
stale.accept(payload(1,{}, {kind:'RECORDED',status:'stale',age_ms:5001}),0);
assert.equal(stale.history.length,0);
assert.equal(stale.view(0).status,'stale');
stale.accept(payload(2,{}, {kind:'RECORDED',status:'recorded',job_status:'UNKNOWN',age_ms:5001}),100);
assert.equal(stale.view(100).status,'stale','unknown job was reported as a known end');
const missing = new NeuralActivityState('genome-a');
missing.accept(payload(1,{}, {kind:'UNAVAILABLE',status:'unavailable',frame:null,age_ms:null}),0);
assert.equal(missing.frame,null);
assert.equal(missing.history.length,0);
""")


def test_schema_individual_and_sample_bounds_are_checked_and_history_is_bounded():
    run_js(r"""
for (const bad of [payload(1,{genome_id:'other'}),payload(1,{experiment_id:'other'}),
    payload(1,{neuron_indices:[0,0,1,2]}),payload(1,{spike_counts:[0,-1,0,0]}),
    payload(1,{neuron_indices:[0,50,12,99]}),payload(1,{evaluation_pass:'0'}),payload(1,{evaluation_pass:65}),
    payload(1,{spike_counts:[0,1.1,0,0]}),payload(1,{window_end_ms:10}),
    payload(1,{neuron_indices:[0,1,2,100]}),payload(1,{n_base:101}),
    payload(1,{neuron_indices:Array.from({length:513},(_,i)=>i),spike_counts:Array(513).fill(0),n_neurons:600}),
    payload(1,{}, {genome_id:'other'}),payload(1,{}, {experiment_id:'other'}),payload(1,{}, {schema_version:2})]) {
  assert.throws(()=>new NeuralActivityState('genome-a','experiment-a').accept(bad,0));
}
const state = new NeuralActivityState('genome-a','experiment-a');
for(let sequence=1;sequence<=75;sequence++) state.accept(payload(sequence),sequence*100);
assert.equal(state.history.length,48);
assert.equal(state.history[0].sequence,28);
assert.equal(state.history[47].sequence,75);
""")


def test_polling_is_sequential_and_watchdog_expires_live_without_a_new_response():
    run_js(r"""
const calls = [];
fetch = (url,options) => { const pending = deferred(); calls.push({url,options,...pending}); return pending.promise; };
const polls = () => calls.filter(c=>c.url.includes('/neural-activity'));
const cleanup = watchNeuralActivity('genome-a',routeGeneration,'experiment-a');
assert.equal(polls().length,1);
assert.equal(timerCount(750),0,'another poll started before the first fetch completed');
assert.ok(polls()[0].url.endsWith('/genome-a/neural-activity'));
assert.equal(polls()[0].options.cache,'no-store');
polls()[0].resolve(response(payload(1))); await settle();
assert.ok(element('#neural-dot').className.includes('is-live'));
assert.equal(timerCount(750),1);
clock.now = 6000; fireTimer(250);
assert.ok(!element('#neural-dot').className.includes('is-live'),'no watchdog expiry while fetches are quiet');
assert.equal(element('#neural-status').textContent,t('n','stale'));
fireTimer(750); assert.equal(polls().length,2);
assert.equal(timerCount(750),0);
polls()[1].reject(new Error('network failed')); await settle();
assert.ok(element('#neural-note').textContent.includes('ゼロ発火ではありません'));
assert.equal(element('#neural-grid').neuralCells[2].count,3);
cleanup();
assert.equal(timerCount(750),0); assert.equal(timerCount(250),0);
""")


def test_route_change_aborts_activity_fetch_and_rejects_its_late_response():
    run_js(r"""
const calls = [];
fetch = (url,options) => { const pending = deferred(); calls.push({url,options,...pending}); return pending.promise; };
routeCleanup = watchNeuralActivity('genome-a',routeGeneration,'experiment-a');
const drawingBefore = element('#neural-grid').drawing.length;
routes.organism = async()=>{};
location.hash = '#/organism/genome-b'; route();
assert.equal(calls[0].options.signal.aborted,true);
calls[0].resolve(response(payload(1))); await settle();
assert.equal(element('#neural-grid').drawing.length,drawingBefore);
assert.equal(timerCount(750),0); assert.equal(timerCount(250),0);
assert.equal(element('#main').innerHTML,`<p class="muted">${t('n','loading')}</p>`);
""")


def test_both_organism_detail_awaits_refuse_old_route_writes():
    run_js(r"""
const calls = [];
fetch = (url,options) => { const pending = deferred(); calls.push({url,options,...pending}); return pending.promise; };
let firstError = null;
const first = organism('genome-a',routeGeneration).catch(error=>{firstError=error;});
routes.organism = async()=>{}; location.hash = '#/organism/genome-b'; route();
calls[0].resolve(response(detail())); await first;
assert.equal(firstError.name,'AbortError');
assert.ok(!element('#main').innerHTML.includes('genome-a'));

const second = organism('genome-a',routeGeneration);
calls[1].resolve(response(detail())); await settle();
assert.equal(calls.length,4,'detail should start activity and M1 requests');
assert.ok(element('#main').innerHTML.includes('neural-activity'));
const activity = calls.find(call=>call.url.endsWith('/neural-activity'));
const metrics = calls.find(call=>call.url.endsWith('/individual/genome-a'));
location.hash = '#/organism/genome-b'; route();
const before = element('#main').innerHTML;
metrics.resolve(response({individual:{genome_id:'genome-a'}}));
activity.resolve(response(payload(1))); await second; await settle();
assert.equal(element('#main').innerHTML,before);
assert.equal(element('#main').appended.length,0);
assert.equal(activity.options.signal.aborted,true);
""")
