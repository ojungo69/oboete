import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

// The same small DOM surface used by the W5B/W5C Node checks.
class Node {
  constructor(tag='div') {
    this.tagName=tag.toUpperCase();this.children=[];this.dataset={};this.listeners={};
    this.className='';this.value='';this.disabled=false;this.hidden=false;this.attrs={};
    this.classList={toggle(){},add(){},remove(){},contains(){return false;}};
  }
  append(...nodes){for(const node of nodes){this.children.push(node);if(node&&typeof node==='object')node.parentElement=this;}}
  replaceChildren(...nodes){this.children=[];this.append(...nodes);}
  addEventListener(name,cb){(this.listeners[name]??=[]).push(cb);}
  setAttribute(name,value){this.attrs[name]=value;}
  removeAttribute(name){delete this.attrs[name];}
  querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
  querySelectorAll(selector){
    const tag=selector.toUpperCase();
    return this.children.filter(node=>node&&typeof node==='object').flatMap(node=>[
      ...(node.tagName===tag||selector.startsWith('.')&&node.className.split(' ').includes(selector.slice(1))?[node]:[]),
      ...node.querySelectorAll(selector)]);
  }
  contains(node){return this===node||this.children.some(child=>child&&typeof child==='object'&&child.contains(node));}
  get childNodes(){return this.children;}
  click(){for(const cb of this.listeners.click||[])cb({target:this});}
  focus(){this.focused=true;}
}

const ids=new Map();
const document={activeElement:null,createElement:tag=>new Node(tag),getElementById:id=>{
  if(!ids.has(id))ids.set(id,new Node());return ids.get(id);
}};
const ctx=vm.createContext({document,location:{hash:''},URLSearchParams,navigator:{language:'en'},
  localStorage:{getItem(){return null;},setItem(){}},window:{addEventListener(){}},
  setInterval(){},TextEncoder,Uint8Array,console});
const source=fs.readFileSync(process.argv[2]||new URL('../../../assets/viewer/app.js',import.meta.url),'utf8');
vm.runInContext(source.replace('await start();','')+'\n'+
  'globalThis.ui={showSettings,drawSettings,setView:v=>{view=v;},setForm:v=>{form=v;},getForm:()=>form,setLang:v=>{lang=v;}};',ctx);
const text=node=>typeof node==='string'?node:(node?.children||[]).map(text).join(' ');
const response=value=>({ok:true,status:200,json:async()=>value});
const rows=(state)=>['claude','codex','grok','agy','opencode','pi','cursor'].map((agent,index)=>({
  agent,launch_file_found:index===0,directory_found:index<2,
  capture:{kind:['hooks','hooks','hooks','hooks','plugin','extension','hooks'][index],
    state:['registered','partial','stale','disabled','invalid','unreadable','unavailable'][index],
    matches_current:[null,false,false,null,null,null,null][index]},
  mcp:{state:index===5?'not_applicable':index===0?'registered':'missing',matches_current:index===0?true:null},
  trust:index===1?state:'not_applicable',live_verified:false,
}));
const report=(trust)=>({home:'missing',config:'invalid',agents:rows(trust)});
const pending=[];let setupGets=0,posts=0;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST')posts++;
  if(url.startsWith('/api/setup?')){setupGets++;return new Promise(resolve=>pending.push(resolve));}
  if(url.startsWith('/api/settings?'))return Promise.resolve(response({error:true}));
  if(url.startsWith('/api/privacy?'))return Promise.resolve(response({available:false,repositories:[],rescan:{state:'unavailable'}}));
  throw new Error('unexpected GET');
};
ctx.ui.setView('settings');
const render=await ctx.ui.showSettings();render();
const panel=document.getElementById('panel');
let section=panel.querySelector('.agent-readiness');
assert(section,'Settings shows agent file inventory even with invalid config');
assert.equal(setupGets,1,'opening Settings passively reads the typed inventory once');
pending.shift()(response(report('matching')));
await new Promise(resolve=>setImmediate(resolve));
assert.equal(section.querySelectorAll('li').length,7,'all seven agents remain visible');
let shown=text(section);
for(const fragment of ['Claude Code','Codex','Grok','Antigravity','OpenCode','Pi','Cursor',
  'registered in file','partly registered','out of date','disabled','invalid file','unreadable','cannot assess',
  'not applicable','current-installation comparison unavailable','Live use: not checked',
  'Memory folder: missing','Saved configuration: invalid file',
  'Launch file: found; login and live use untested','Settings folder: found',
  'Use Run diagnostics below for the current read-only checks']){
  assert(shown.includes(fragment),`fixed state visible: ${fragment}`);
}
assert(!shown.includes('healthy')&&!shown.includes('ready'),'file inventory does not claim live readiness');

ctx.ui.setLang('ja');ctx.ui.drawSettings();section=panel.querySelector('.agent-readiness');
shown=text(section);
assert(shown.includes('エージェントの登録状況')&&shown.includes('実際の利用：未確認'),'Japanese inventory is drawn');
ctx.ui.setLang('en');ctx.ui.drawSettings();section=panel.querySelector('.agent-readiness');
const draft=new Node('input');draft.value='unsaved local draft';panel.querySelector('.settings').append(draft);
const mine={draft:'unsaved local draft'};ctx.ui.setForm(mine);
const refresh=()=>section.querySelectorAll('button').find(node=>node.dataset.action==='agent_inventory.refresh');
refresh().click();refresh().click();
assert.equal(setupGets,3,'explicit refresh uses GET and allows a newer read');
pending.pop()(response(report('stale')));
await new Promise(resolve=>setImmediate(resolve));
pending.pop()(response(report('matching')));
await new Promise(resolve=>setImmediate(resolve));
shown=text(section);
assert(shown.includes('Hook trust: out of date')&&!shown.includes('Hook trust: matching'),'late response cannot replace newer result');
assert.equal(draft.value,'unsaved local draft');
assert.equal(ctx.ui.getForm(),mine,'readiness refresh preserves the settings draft');
refresh().click();
const beforeLeaving=text(section);
ctx.ui.setView('records');
pending.pop()(response(report('matching')));
await new Promise(resolve=>setImmediate(resolve));
assert.equal(text(section),beforeLeaving,'a GET result after leaving Settings is discarded');
ctx.ui.setView('settings');
refresh().click();
const privateMarker='SyntheticUnexpectedReadinessValue';
const malformed=report(privateMarker);
malformed.agents[0].launch_file_found=null;
malformed.agents[0].directory_found='unexpected';
malformed.agents[0].capture.kind={toString:'not callable'};
malformed.agents[0].capture.state=privateMarker;
malformed.agents[0].live_verified=true;
pending.pop()(response(malformed));
await new Promise(resolve=>setImmediate(resolve));
shown=text(section);
assert(shown.includes('Launch file: Unknown')&&shown.includes('Settings folder: Unknown'),'unknown flags are not treated as absent');
assert(shown.includes('Live use: Unknown')&&!shown.includes(privateMarker),'unrecognized states never become live verification or raw text');
assert.equal(ctx.ui.getForm(),mine,'unknown report fields preserve the draft');
assert.equal(posts,0,'passive and explicit inventory reads never POST');
ctx.ui.setLang('en');
assert.equal(text(vm.runInContext('keyState({key:"unknown"})[0]',ctx)),
  'Installation could not be checked','unknown CLI status has an English label');
ctx.ui.setLang('ja');
assert.equal(text(vm.runInContext('keyState({key:"unknown"})[0]',ctx)),
  'インストール状況は確認できません','unknown CLI status has a Japanese label');
vm.runInContext('globalThis.doctorUi={runDoctor,redraw:renderDoctor};',ctx);
ctx.ui.setLang('en');ctx.ui.setForm(null);ctx.ui.drawSettings();
let diagnostics=panel.querySelector('.doctor-diagnostics');
assert(diagnostics,'invalid configuration still offers explicit diagnostics');
const diagnosticDraft=new Node('input');diagnosticDraft.value='keep this unsaved value';
panel.querySelector('.settings').append(diagnosticDraft);
const diagnosticRequests=[];
ctx.fetch=(url,options)=>{
  assert.equal(url,'/api/doctor');assert.equal(options.method,'POST');assert.equal(options.body,'{}');
  return new Promise((resolve,reject)=>diagnosticRequests.push({resolve,reject}));
};
const count=value=>({state:'known',value});
const absent={state:'absent',value:null};
const diagnosticReport={complete:false,unhealthy:['raw_damaged'],inventory:report('stale'),stores:{},checks:{
  source_stability:'known',remaining:'none',
  raw:{integrity:'damaged',max_seq:absent,max_op_seq:absent,curated_through:absent,parking:{state:'absent'}},
  knowledge:{integrity:'known',rewinds:{count:count(0),last_at_ms:null},gaps:{state:'known',rows:[]}},
  legacy:{integrity:'absent'},embeddings:{state:'off'},
  providers:{integrity:'absent'},
  disk:{state:'known',free_bytes:0,low_space:true},
  retained:{state:'known',categories:[{category:'memory_database',present:{state:'known',value:true},targets:count(1),bytes:count(0)}],
    evaluation_copies:{present:{state:'known',value:false},bytes:count(0)}}
}};
const diagnosticResponse=(value,status=200)=>({ok:status===200,status,headers:{get:()=> 'application/json'},json:async()=>value});
const firstDiagnostic=ctx.doctorUi.runDoctor();
await ctx.doctorUi.runDoctor();
assert.equal(diagnosticRequests.length,1,'a second click while running sends no second POST');
assert(diagnostics.querySelector('button').disabled,'diagnostic button disables while pending');
diagnosticRequests.shift().resolve(diagnosticResponse(diagnosticReport));
await firstDiagnostic;
shown=text(diagnostics);
assert(shown.includes('Saved records are damaged')&&shown.includes('Available bytes 0'),
  'known problems and an established zero remain visible');
assert(shown.includes('logical bytes 0')&&shown.includes('present yes'),
  'retained empty files are distinct from missing and unknown');
assert(!shown.includes('does not implement every diagnostic'),'remaining none hides the obsolete unimplemented notice');
assert.equal(diagnosticDraft.value,'keep this unsaved value');
assert(panel.contains(diagnosticDraft),'diagnostics replaces only its section');
ctx.ui.setLang('ja');ctx.doctorUi.redraw();shown=text(diagnostics);
assert(shown.includes('保存された記録が破損')&&shown.includes('利用可能な容量0'),
  'Japanese diagnostics displays fixed labels and known zero');
ctx.ui.setLang('en');
const busy=ctx.doctorUi.runDoctor();diagnosticRequests.shift().resolve(diagnosticResponse({},503));await busy;
assert(text(diagnostics).includes('viewer is busy'),'busy response is explained');
await new Promise(resolve=>setImmediate(resolve));
assert.equal(diagnosticRequests.length,0,'busy response is not automatically retried');
const lost=ctx.doctorUi.runDoctor();diagnosticRequests.shift().reject(new Error('invented transport failure'));await lost;
assert(text(diagnostics).includes('response was lost'),'lost response is unknown');
await new Promise(resolve=>setImmediate(resolve));
assert.equal(diagnosticRequests.length,0,'lost response is not automatically retried');
assert(panel.contains(diagnosticDraft),'failure paths preserve the draft node');
const late=ctx.doctorUi.runDoctor();const beforeDoctorLeaving=text(diagnostics);
ctx.ui.setView('records');diagnosticRequests.shift().resolve(diagnosticResponse(diagnosticReport));await late;
assert.equal(text(diagnostics),beforeDoctorLeaving,'result after leaving Settings does not redraw');
ctx.ui.setView('settings');ctx.ui.drawSettings();
diagnostics=panel.querySelector('.doctor-diagnostics');
assert(text(diagnostics).includes('Saved records are damaged'),
  'returning to Settings retains the completed diagnostic result');
const lostAway=ctx.doctorUi.runDoctor();ctx.ui.setView('records');
diagnosticRequests.shift().reject(new Error('invented off-view transport failure'));await lostAway;
ctx.ui.setView('settings');ctx.ui.drawSettings();
diagnostics=panel.querySelector('.doctor-diagnostics');
assert(text(diagnostics).includes('response was lost'),
  'returning to Settings retains the matching lost-response state');
diagnosticReport.inventory=report('matching');
const freshInventory=ctx.doctorUi.runDoctor();
diagnosticRequests.shift().resolve(diagnosticResponse(diagnosticReport));await freshInventory;
assert(text(diagnostics).includes('Agent files in this diagnostic check'),
  'the result renders its own captured agent inventory');
assert(text(diagnostics).includes('Hook trust: matching'),
  'the result uses the captured inventory rather than an older independent GET');
console.log('PASS: seven fixed agent rows, JA/EN, latest GET, explicit Doctor POST, failure handling and unchanged draft');
