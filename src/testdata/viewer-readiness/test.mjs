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
  // A browser moves the focus to the body when the focused element leaves the page (#810), and
  // Chrome blurs it as it goes, the element still in the page then (69b-3c).
  replaceChildren(...nodes){
    if(this.children.some(c=>c&&typeof c==='object'&&!nodes.includes(c)&&c.contains(document.activeElement)))blurOff();
    this.children=[];this.append(...nodes);
  }
  remove(){
    if(document.activeElement&&document.activeElement!==document.body&&this.contains(document.activeElement))blurOff();
    if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(node=>node!==this);this.parentElement=null;
  }
  addEventListener(name,cb){(this.listeners[name]??=[]).push(cb);}
  setAttribute(name,value){this.attrs[name]=value;}
  removeAttribute(name){delete this.attrs[name];}
  querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
  // In document order, as a browser gives them, a list of selectors too (Codex's review of 69b-2a).
  querySelectorAll(selector){
    const parts=selector.split(',').map(part=>part.trim());
    const matches=node=>parts.some(part=>node.tagName===part.toUpperCase()||part.startsWith('.')&&node.className.split(' ').includes(part.slice(1)));
    return this.children.filter(node=>node&&typeof node==='object').flatMap(node=>[...(matches(node)?[node]:[]),...node.querySelectorAll(selector)]);
  }
  contains(node){return this===node||this.children.some(child=>child&&typeof child==='object'&&child.contains(node));}
  get childNodes(){return this.children;}
  get options(){return this.children.filter(node=>node?.tagName==='OPTION');}
  click(){for(const cb of this.listeners.click||[])cb({target:this});}
  focus(){this.focused=true;document.activeElement=this;}
  // A number field has no selection: a browser throws, and reads none (69b-3c).
  setSelectionRange(start,end,direction){
    if(this.type==='number')throw new Error('InvalidStateError');
    Object.assign(this,{selectionStart:start,selectionEnd:end,selectionDirection:direction});
  }
  select(){this.selected=true;}
  closest(selector){
    for(let node=this;node;node=node.parentElement){
      if(node.tagName===selector.toUpperCase()||selector.startsWith('.')&&node.className.split(' ').includes(selector.slice(1)))return node;
    }
    return null;
  }
}

// The focus taken off the element leaving the page, its blur and focusout run as Chrome runs them,
// the focusout up through its ancestors (69b-3c).
function blurOff(){
  const gone=document.activeElement;
  document.activeElement=document.body;
  for(const cb of gone?.listeners?.blur||[])cb({target:gone,relatedTarget:null});
  for(let node=gone;node&&node!==document.body;node=node.parentElement)
    for(const cb of node.listeners?.focusout||[])cb({target:gone,relatedTarget:null});
}
const ids=new Map();
const document={activeElement:null,createElement:tag=>new Node(tag),getElementById:id=>{
  if(!ids.has(id))ids.set(id,new Node());return ids.get(id);
}};
// Where a browser puts the focus taken off an element (#810).
document.body=new Node('body');
const ctx=vm.createContext({document,location:{hash:'#t=0123456789abcdef0123456789abcdef',port:'17373',
  replaced:[],replace(url){this.replaced.push(url);}},URL,URLSearchParams,navigator:{language:'en'},
  localStorage:{getItem(){return null;},setItem(){}},window:{addEventListener(){}},
  setInterval(){},TextEncoder,Uint8Array,console});
const sourcePath=fs.realpathSync(process.argv[2]||new URL('../../../assets/viewer/app.js',import.meta.url));
const source=fs.readFileSync(sourcePath,'utf8');
vm.runInContext(source.replace('await start();',''),ctx,{filename:sourcePath});
vm.runInContext('globalThis.ui={showSettings,drawSettings,show,setView:v=>{view=v;},setForm:v=>{form=v;},getForm:()=>form,setLang:v=>{lang=v;}};',ctx);
// The view the page opens on, before the checks below change it.
const initialView=vm.runInContext('view',ctx);
// A whole settings answer, set by the W4 checks below for the later ones that open Settings.
let validSettings=null;
// The simple page uses the saved chain, including unavailable and disabled entries.
{
const simple=vm.runInContext('({status:simpleProviderStatus,models:simpleProviderModels,listed:catalogHasProvider,chain:simpleSavedChain,catalog:AI_CATALOG})',ctx);
const row={name:'groq',key:'ok',on:true,effective_model:'openai/gpt-oss-20b',model_rule:'any'};
assert.equal(simple.status(row,[]),'ready');
assert.equal(simple.status({...row,key:'missing',on:false},['groq']),'missing','a missing key is explained before other states');
assert.equal(simple.status({...row,key:'not-on-path'},['groq']),'not-installed');
assert.equal(simple.status({...row,on:false},['groq']),'stopped','an owner stop retains its resume path');
assert.equal(simple.status({...row,on:false},[]),'off');
assert.equal(simple.status({...row,key:'unknown'},[]),'unknown','unknown installation is never ready');
assert.equal(simple.status(row,null),'unknown','unreadable stops are never reported as ready');
const preset={savedSettings:{chain:[{...row,key:'none'}]},providers:[{name:'groq',saved:{enabled:true,base_url:'https://api.groq.com/openai/v1'}}]};
assert.equal(simple.chain(preset)[0].key,'missing','a newly added keyed preset still needs its key after reload');
assert.equal(simple.status(simple.chain(preset)[0],[]),'missing');
preset.providers[0].saved.base_url='http://127.0.0.1:11434/v1';
assert.equal(simple.chain(preset)[0].key,'none','a custom keyless server stays keyless');
preset.providers[0].saved.enabled=false;
assert.equal(simple.status(simple.chain(preset)[0],[]),'off','native disabling remains distinct from a name overlay');
const models=(value)=>Array.from(simple.models(value,['openai/gpt-oss-120b','openai/gpt-oss-20b','openai/gpt-oss-120b']));
assert.deepEqual(models(row),['openai/gpt-oss-20b','openai/gpt-oss-120b'],'the effective model comes first, without duplicates');
assert.deepEqual(models({...row,effective_model:'custom-owned-model'}),['custom-owned-model','openai/gpt-oss-120b','openai/gpt-oss-20b']);
assert.deepEqual(models({...row,model_rule:'fixed'}),[],'priced models are edited together with their prices in detail');
assert.deepEqual(Array.from(simple.models({...row,model_rule:'free',effective_model:'current:free'},
  ['paid-model','other:free','misleading:free-extra','current:free'])),['current:free','other:free']);
assert.deepEqual(Array.from(simple.models({...row,model_rule:'free',effective_model:'paid-model'},['other:free'])),['other:free']);
assert.deepEqual(Array.from(simple.models({...row,effective_model:null},[])),[],'a CLI default needs no invented model name');
assert(simple.listed({name:'groq'},[{name:'groq',on:false}]),'an off entry is already in the list');
assert(!simple.listed({name:'groq'},[{name:'groq-20b'}]),'similar names are separate entries');
assert(!simple.listed({name:''},[{name:''}]),'the custom server is never a duplicate');
assert.equal(new Set(simple.catalog.filter(entry=>entry.name).map(entry=>entry.name)).size,simple.catalog.length-1);
}
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
assert.equal(text(vm.runInContext('note(t("key_unknown"))',ctx)),
  'Installation could not be checked','unknown CLI status has an English label');
ctx.ui.setLang('ja');
assert.equal(text(vm.runInContext('note(t("key_unknown"))',ctx)),
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
let integration=panel.querySelector('.agent-setup');
assert(integration,'Settings offers confirmed agent connections separately from diagnostics');
vm.runInContext('globalThis.setupUi={draft:agentSetupDraft,preview:previewAgentSetup,start:startAgentSetup,inspect:inspectAgentSetup,render:renderAgentSetup};',ctx);
let operationNonce=17;
ctx.crypto={getRandomValues:bytes=>bytes.fill(operationNonce++)};
const setupRequests=[];
ctx.fetch=(url,options)=>new Promise((resolve,reject)=>setupRequests.push({url,options,resolve,reject}));
const setupFields=()=>integration.querySelectorAll('input').filter(node=>node.dataset.field?.startsWith('agent_setup.agent.'));
assert.equal(setupFields().length,7,'all seven integrations can be selected');
const integrationDraft=new Node('input');integrationDraft.value='preserve agent-operation draft';
panel.querySelector('.settings').append(integrationDraft);
const change=(node,checked)=>{node.checked=checked;for(const cb of node.listeners.change||[])cb({target:node});};
change(setupFields().find(node=>node.dataset.field.endsWith('.claude')),true);
await ctx.setupUi.start();
assert.equal(setupRequests.length,0,'selection alone cannot apply an integration');
const selectedPreview={preview_key:'a'.repeat(64),action:'wire',live_verified:false,activation:'next_session',
  agents:[{agent:'claude',steps:[{component:'mcp',effect:'native_command',backup:'none'}]}]};
const operationReceipt=id=>({...selectedPreview,operation_id:id,phase:'complete',agents:[{agent:'claude',
  steps:[{component:'mcp',effect:'native_command',backup:'none',kind:'native_add',outcome:'committed'}]}]});
const prepare=async(value=selectedPreview)=>{
  const task=ctx.setupUi.preview();
  const pending=setupRequests.shift();assert(pending,'preview makes its explicit request');
  assert.equal(pending.url,'/api/setup/preview');
  assert.deepEqual(JSON.parse(pending.options.body),{action:'wire',agents:['claude']});
  pending.resolve(diagnosticResponse(value));await task;
};
await prepare();
assert.equal(ctx.setupUi.draft.confirmed,false,'a preview never checks consent');
await ctx.setupUi.start();assert.equal(setupRequests.length,0,'a preview without consent cannot apply');
const consent=()=>integration.querySelectorAll('input').find(node=>node.dataset.field==='agent_setup.confirmed');
change(consent(),true);
const completed=ctx.setupUi.start();const firstApply=setupRequests.shift();
assert.equal(firstApply.url,'/api/setup/start');
const firstBody=JSON.parse(firstApply.options.body);
assert.equal(firstBody.confirmed,true);assert.equal(firstBody.operation_id,'11'.repeat(32));
const beforeAgentLeaving=text(integration);ctx.ui.setView('records');
firstApply.resolve(diagnosticResponse(operationReceipt(firstBody.operation_id)));await completed;
assert.equal(text(integration),beforeAgentLeaving,'completion away from Settings does not change the current panel');
ctx.ui.setView('settings');ctx.setupUi.render();
assert(text(integration).includes('complete')&&text(integration).includes('Current operation'),
  'returning retains the completed current operation');
assert(panel.contains(integrationDraft)&&integrationDraft.value==='preserve agent-operation draft',
  'preview and apply only replace their own section');
ctx.ui.setLang('ja');ctx.setupUi.render();
assert(text(integration).includes('エージェントの接続と解除')&&text(integration).includes('実際の利用は未確認'),
  'Japanese connection and honest activation labels are shown');
ctx.ui.setLang('en');ctx.setupUi.render();
await prepare();change(consent(),true);
const busyStart=ctx.setupUi.start();
assert.equal(integration.querySelector('.agent-setup-receipt'),null,
  'a newly admitted start does not show the previous completion');
assert(text(integration).includes('Applying selected agent changes'),
  'the pending start has a fixed sending label');
setupRequests.shift().resolve(diagnosticResponse({},503));await busyStart;
assert.equal(ctx.setupUi.draft.unknown,false,'a definite busy refusal does not become an unknown mutation');
assert(text(integration).includes('Another operation is active'));
await prepare();change(consent(),true);
const lostStart=ctx.setupUi.start();setupRequests.shift().reject(new Error('invented agent response loss'));await lostStart;
assert.equal(ctx.setupUi.draft.unknown,true);
await ctx.setupUi.start();await ctx.setupUi.preview();
assert.equal(setupRequests.length,0,'lost operation is neither retried nor replaced automatically');
const priorStatus=ctx.setupUi.inspect();
setupRequests.shift().resolve(response({active:null,last:operationReceipt('22'.repeat(32))}));await priorStatus;
assert.equal(ctx.setupUi.draft.unknown,true,'a previous operation cannot resolve the pending one');
assert(text(integration).includes('Previous operation')&&!text(integration).includes('Current operation'),
  'a nonmatching last receipt is labelled as previous');
const inspect=ctx.setupUi.inspect();const inspectRequest=setupRequests.shift();
assert.equal(inspectRequest.url,'/api/setup/operation?');
inspectRequest.resolve(response({active:null,last:operationReceipt(ctx.setupUi.draft.operationId)}));await inspect;
assert.equal(ctx.setupUi.draft.unknown,false,'explicit matching status inspection recovers a lost response');
assert(text(integration).includes('Current operation'),
  'a matching last receipt is labelled as the current operation');
const completedStatus=ctx.setupUi.draft.status;
ctx.setupUi.draft.status={active:{...operationReceipt(ctx.setupUi.draft.operationId),phase:'running'},last:null};
ctx.setupUi.render();
assert(text(integration).includes('Current operation'),
  'an active receipt is labelled as the current operation');
ctx.setupUi.draft.status=completedStatus;ctx.setupUi.render();
const priorReceipt=ctx.setupUi.draft.receipt;
const badStatus=ctx.setupUi.inspect();setupRequests.shift().resolve(response({active:'private-state-canary',last:null}));await badStatus;
assert.equal(ctx.setupUi.draft.receipt,priorReceipt,'malformed status retains the established receipt');
assert(!text(integration).includes('private-state-canary'));
for(const whileSending of [false,true]) {
  await prepare();change(consent(),true);
  let staleRead,staleRequest;
  if(!whileSending){staleRead=ctx.setupUi.inspect();staleRequest=setupRequests.shift();}
  const applying=ctx.setupUi.start();const applyRequest=setupRequests.shift();
  const body=JSON.parse(applyRequest.options.body);
  if(whileSending){staleRead=ctx.setupUi.inspect();staleRequest=setupRequests.shift();}
  const completeReceipt=operationReceipt(body.operation_id);
  applyRequest.resolve(diagnosticResponse(completeReceipt));await applying;
  staleRequest.resolve(response(whileSending
    ? {active:{...completeReceipt,phase:'running'},last:null}
    : {active:null,last:priorReceipt}));
  await staleRead;
  assert.equal(ctx.setupUi.draft.status.last?.operation_id,body.operation_id,
    `a late status read begun ${whileSending?'during':'before'} apply cannot replace completion`);
  assert.equal(ctx.setupUi.draft.status.active,null,'a late running response cannot leave setup blocked');
  assert.equal(ctx.setupUi.draft.reading,false,'invalidating a read releases its loading state');
}
await prepare();change(consent(),true);
const unknownStart=ctx.setupUi.start();setupRequests.shift().reject(new Error('invented unknown result'));await unknownStart;
const prepareAnother=()=>integration.querySelectorAll('button').find(node=>node.dataset.action==='agent_setup.prepare');
assert(!prepareAnother(),'a lost response alone cannot admit a new operation');
const unknownReceipt={...operationReceipt(ctx.setupUi.draft.operationId),phase:'unknown'};
const inspectActive=ctx.setupUi.inspect();
setupRequests.shift().resolve(response({active:{...operationReceipt('ff'.repeat(32)),phase:'running'},last:unknownReceipt}));await inspectActive;
assert(!prepareAnother(),'an active operation prevents preparing another, even after inspecting a matching unknown receipt');
const inspectUnknown=ctx.setupUi.inspect();
setupRequests.shift().resolve(response({active:null,last:unknownReceipt}));await inspectUnknown;
assert.equal(ctx.setupUi.draft.receipt?.phase,'unknown','explicit inspection retains the matching unknown receipt');
assert.equal(ctx.setupUi.draft.unknown,true,'inspection does not claim an unknown outcome is resolved');
assert(prepareAnother()&&!prepareAnother().disabled,'confirmed inactive unknown operation offers explicit new preparation');
prepareAnother().click();
assert.equal(ctx.setupUi.draft.unknown,false);
assert.equal(ctx.setupUi.draft.operationId,null);
assert.equal(ctx.setupUi.draft.confirmed,false);
assert.equal(ctx.setupUi.draft.preview,null);
assert(text(integration).includes('Previous operation')&&text(integration).includes('unknown'),
  'new preparation retains the previous uncertain result');
assert.equal(setupRequests.length,0,'preparing another operation never automatically POSTs');
await ctx.setupUi.start();assert.equal(setupRequests.length,0,'a fresh preview and consent are required');
await prepare({...selectedPreview,agents:[{agent:'claude',steps:[{component:'mcp',effect:'private-effect-canary',backup:'none'}]}]});
assert.equal(ctx.setupUi.draft.preview,null,'unknown effects cannot be approved');
assert(!text(integration).includes('private-effect-canary'));
assert.equal(setupRequests.length,0,'rendering and status recovery never auto POST');
console.log('PASS: seven agents, JA/EN, explicit Doctor, confirmed integration, off-tab completion, busy/lost recovery and unchanged drafts');

vm.runInContext('globalThis.recoveryUi={draft:recoveryDraft,action:recoveryAction};',ctx);
ctx.ui.setView('settings');ctx.ui.setForm(null);ctx.ui.setLang('en');ctx.ui.drawSettings();
let recovery=()=>panel.querySelector('.settings-recovery');
assert(text(recovery()).includes('custom rules and other settings return to defaults'));
ctx.ui.setLang('ja');ctx.ui.drawSettings();
assert(text(recovery()).includes('独自ルールや設定は初期値に戻ります'));
const recoveryRequests=[];
ctx.fetch=(url,options)=>new Promise((resolve,reject)=>recoveryRequests.push({url,options,resolve,reject}));
assert.equal(recoveryRequests.length,0,'drawing recovery sends no request');
const recoveryPreview={preview_key:'a'.repeat(64),replaces:'all_settings',copy:'current_bytes',
  ai:'off',prompt_text:'off',injection:'off',other_capture:'builtin_redaction'};
const previewRecovery=async(answer=recoveryPreview)=>{
  const waiting=ctx.recoveryUi.action('preview');
  const request=recoveryRequests.shift();
  assert.equal(request.url,'/api/settings/recovery/preview');
  assert.equal(request.options.body,'{}');
  request.resolve(diagnosticResponse(answer));await waiting;
};
await previewRecovery();
assert.equal(ctx.recoveryUi.draft.confirmed,false,'a fresh preview has unchecked consent');
await ctx.recoveryUi.action('start');assert.equal(recoveryRequests.length,0);
const recoveryConsent=()=>recovery().querySelectorAll('input').find(node=>node.dataset.field==='recovery.confirmed');
change(recoveryConsent(),true);
const recoveryStart=ctx.recoveryUi.action('start');
await ctx.recoveryUi.action('start');
assert.equal(recoveryRequests.length,1,'recovery has one explicit in-flight POST');
const recoveryRequest=recoveryRequests.shift();
assert.equal(recoveryRequest.url,'/api/settings/recovery/start');
assert.deepEqual(JSON.parse(recoveryRequest.options.body),{preview_key:'a'.repeat(64),confirmed:true});
recoveryRequest.reject(new Error('invented lost result'));await recoveryStart;
assert.equal(ctx.recoveryUi.draft.unknown,true);
assert.equal(ctx.recoveryUi.draft.preview,null);
await ctx.recoveryUi.action('start');assert.equal(recoveryRequests.length,0,'a lost result is never resent');
const blockedRecoveryPreview=ctx.recoveryUi.action('preview');
assert.equal(recoveryRequests.length,0,'inspect saved settings before preparing another uncertain recovery');
await blockedRecoveryPreview;
const inspectRecovery=ctx.recoveryUi.action('inspect');
const inspectRecoveryRequest=recoveryRequests.shift();
assert(inspectRecoveryRequest.url.startsWith('/api/settings?'));
assert(!inspectRecoveryRequest.options?.method,'explicit inspection only reads settings');
inspectRecoveryRequest.resolve(response({error:'file_invalid'}));await inspectRecovery;
assert.equal(recoveryRequests.length,0,'inspection never starts recovery');
await previewRecovery({...recoveryPreview,other_capture:'private-recovery-canary'});
assert.equal(ctx.recoveryUi.draft.preview,null,'unknown effects cannot receive consent');
assert(!text(recovery()).includes('private-recovery-canary'));
await previewRecovery();change(recoveryConsent(),true);
const nativeUnknown=ctx.recoveryUi.action('start');
const retainedName='config.toml.recovery-'+'b'.repeat(32)+'.bak';
recoveryRequests.shift().resolve(diagnosticResponse({phase:'unknown',backup:retainedName}));await nativeUnknown;
assert.equal(ctx.recoveryUi.draft.unknown,true);
assert.equal(ctx.recoveryUi.draft.receipt,retainedName,'Unknown retains the known private copy');
assert(text(recovery()).includes(retainedName)&&!text(recovery()).includes('Settings recovered'),
  'a retained copy does not claim recovery completed');
console.log('PASS: JA/EN recovery disclosure, explicit preview/consent, single POST, lost-result inspection and fixed response fields');

// The guided choices use the same native Settings payload, with no separate profile.
{
vm.runInContext('globalThis.tier={classify:onboardingClass,groups:onboardingGroups,plan:onboardingPlan,apply:applyOnboardingValues,text:(key,language)=>{lang=language;return t(key);}};',ctx);
const {tier} = ctx;
const limits=(input=0,output=0)=>({usd_per_mtok_in:input,usd_per_mtok_out:output});
const row=(name,kind='openai',extras={})=>({name,selector:{source:extras.source||'file'},
  saved:{kind,enabled:extras.enabled??true,subscription:extras.subscription??false,
    endpoint_supported:extras.endpoint_supported??true,
    cli:extras.cli||'codex',base_url:extras.base_url||'https://billing.example/v1',
    limits:limits(...(extras.price||[0,0]))}});
const providers=[
  row('builtin','openai',{source:'builtin',base_url:'https://api.groq.com/openai/v1'}),
  row('local','openai',{base_url:'http://127.0.0.2:11434/v1'}),
  row('subscription','cli'),
  row('unsafe-cli','cli',{cli:'agy'}),
  row('paid','openai',{price:[0.5,1]}),
  row('remote-unknown'),
  row('mixed','openai',{source:'builtin',base_url:'https://api.groq.com/openai/v1'}),
  row('mixed','openai',{price:[1,2]}),
  row('disabled','openai',{enabled:false,price:[1,2]}),
  row('mixed-disabled','openai',{source:'builtin',base_url:'https://api.groq.com/openai/v1'}),
  row('mixed-disabled','openai',{enabled:false,price:[1,2]}),
];
const names=['builtin','local','subscription','unsafe-cli','paid','remote-unknown','mixed','disabled','mixed-disabled'];
const fixture=()=>({firstRun:true,summary:{curate:false,language:'Japanese'},paid_usd_per_month:'0',
  chain:names.map(name=>({name,edit:{on:true,daily_budget:'7',timeout_s:'90',model:'owned'}})),
  providers,inject:{session_start:true},backup:{edit:'owned'}});
const namesOf=p=>p.names||[];

let f=fixture();
assert.equal(tier.classify(providers[0]),'free');
assert.equal(tier.classify(providers[1]),'free');
assert.equal(tier.classify(row('local-v6','openai',{base_url:'http://[::1]:11434/v1'})),'free');
assert.equal(tier.classify(providers[2]),'subscription');
assert.equal(tier.classify(row('go','openai',{subscription:true})),'subscription');
assert.equal(tier.classify(row('priced-sub','openai',{subscription:true,price:[1,0]})),'paid');
assert.equal(tier.classify(providers[3]),'unknown','unapproved CLI is not preset as a curator');
assert.equal(tier.classify(providers[4]),'paid');
assert.equal(tier.classify(providers[5]),'unknown');
assert.equal(tier.classify(providers[8]),'disabled');
assert.equal(tier.classify(row('hidden-paid','openai',{price:[1,2],endpoint_supported:false})),
  'unknown','a hidden or unsupported destination cannot enter a preset');
assert.equal(tier.classify(row('hidden-subscription','openai',{subscription:true,endpoint_supported:false})),
  'unknown','subscription metadata does not override a refused destination');
assert.equal(tier.classify(row('priced-unsafe-cli','cli',{cli:'agy',price:[1,2]})),
  'unknown','configured prices do not authorize an unsupported CLI');
assert.equal(tier.groups(f).get('mixed'),'paid','one priced same-name entry raises group risk');
assert.equal(tier.groups(f).get('mixed-disabled'),'free','a disabled priced entry is not enabled');
assert.equal(f.summary.curate,false,'planning does not reinterpret the saved default');
assert.deepEqual(Array.from(namesOf(tier.plan(f,'free'))),['builtin','local','mixed-disabled']);
assert.deepEqual(Array.from(namesOf(tier.plan(f,'subscription'))),['builtin','local','subscription','mixed-disabled']);
assert.deepEqual(Array.from(namesOf(tier.plan(f,'paid'))),['builtin','local','subscription','paid','mixed','mixed-disabled']);
assert.equal(tier.plan(f,'paid').cap,'5','paid uses the configured positive cap or USD 5 default');
const capped=fixture();capped.paid_usd_per_month='2.75';
assert.equal(tier.plan(capped,'paid').cap,'2.75','a positive existing cap is preserved');
// Decision 64: a cap turned off stays off where paid calls are chosen; a free tier's 0 turns it on.
const open=fixture();open.paid_usd_per_month='';open.paidNoCap=true;
assert.equal(tier.plan(open,'paid').cap,null,'no cap stays for paid calls');
tier.apply(open,tier.plan(open,'paid'));
assert.deepEqual([open.paidNoCap,open.paid_usd_per_month],[true,'']);
tier.apply(open,tier.plan(open,'free'));
assert.deepEqual([open.paidNoCap,open.paid_usd_per_month],[false,'0'],'a free tier stops paid calls again');
const none=tier.plan(f,'none');tier.apply(f,none);
assert.equal(f.summary.curate,false);
assert.equal(f.paid_usd_per_month,'0');
assert(f.chain.every(entry=>entry.edit.on),'none keeps the chain while curation is off');
const paid=tier.plan(f,'paid');tier.apply(f,paid);
assert.equal(f.summary.curate,true);
assert.equal(f.paid_usd_per_month,'5');
assert.deepEqual(f.chain.filter(entry=>entry.edit.on).map(entry=>entry.name),namesOf(paid));
assert.equal(f.summary.language,'Japanese');
assert(f.chain.every(entry=>entry.edit.model==='owned'&&entry.edit.daily_budget==='7'));
assert.equal(f.inject.session_start,true);assert.equal(f.backup.edit,'owned');

const empty=fixture();empty.providers=[row('remote-unknown')];empty.chain=[empty.chain[5]];
empty.summary.curate=true;empty.paid_usd_per_month='1';
const unavailable=tier.plan(empty,'free');
assert.equal(unavailable.available,false);
tier.apply(empty,unavailable);
assert.equal(empty.summary.curate,false);
assert.equal(empty.paid_usd_per_month,'1');
assert.equal(empty.chain[0].edit.on,true,'no eligible name preserves the chain');
assert(tier.text('onboarding_first_h','en').includes('starting AI tier'));
assert(tier.text('onboarding_first_h','ja').includes('最初のAI利用段階'));
assert(tier.text('onboarding_again_h','ja').includes('変更'));
f=fixture();f.saved={capture:{store_prompts:false}};f.capture={store_prompts:true};
vm.runInContext('globalThis.onboardingPanel=onboardingSection;',ctx);
// CodeRabbit on #772: the guide's effect with no cap for paid calls says none, never USD null.
const unlimited=fixture();unlimited.paid_usd_per_month='';unlimited.paidNoCap=true;
unlimited.saved={capture:{store_prompts:false}};unlimited.capture={store_prompts:false};
ctx.onboardingPanel(unlimited);vm.runInContext("onboardingTier='paid';",ctx);
ctx.ui.setLang('en');const said=text(ctx.onboardingPanel(unlimited));
assert(said.includes('Monthly paid cap: none.')&&!said.includes('null'),'no cap for paid calls is said as none');
ctx.ui.setLang('ja');
assert(text(ctx.onboardingPanel(unlimited)).includes('有料利用の月額上限：なし。'));
ctx.ui.setLang('en');
assert(text(ctx.onboardingPanel(f)).includes('A new typed phrase will not be searchable'),
  'the journey follows saved prompt privacy, including an unsaved opt-in');
assert.equal(f.saved.capture.store_prompts,false,'the guide never opts into prompt storage');
ctx.ui.setLang('ja');
assert(text(ctx.onboardingPanel(f)).includes('入力文を保存しない設定です'));
// A first successful listing must keep the guide's explicit All scope.
document.querySelector=selector=>selector==='main'?new Node('main'):null;
document.querySelectorAll=()=>[];
ctx.Option=class extends Node {
  constructor(label,value){super('option');this.append(label);this.value=value;}
};
f.testPhrase='separate repository needle';ctx.ui.setForm(f);ctx.ui.setView('settings');
vm.runInContext('reposLoaded=false;currentRepo="";',ctx);
const queryRequests=[];
ctx.fetch=(url,options)=>{
  assert(!options?.method,'the record check uses only existing reads');
  if(url.startsWith('/api/repos?'))return Promise.resolve(response({current:'owner/current',repos:[]}));
  if(url.startsWith('/api/search?')){
    queryRequests.push(new URL(url,'http://127.0.0.1'));
    return Promise.resolve(response({hits:[],vector:'off',why:null}));
  }
  throw new Error('unexpected guided search request');
};
const queryButton=ctx.onboardingPanel(f).querySelectorAll('button')
  .find(node=>node.dataset.action==='onboarding.search');
await queryButton.listeners.click[0]({target:queryButton});
assert.equal(queryRequests.length,1,'listing does not first search the current repository');
assert.equal(queryRequests[0].searchParams.get('all'),'1','explicit All survives the first repository listing');
assert.equal(queryRequests[0].searchParams.get('repo'),null);
assert.equal(queryRequests[0].searchParams.get('raw'),'only');
for(const alreadyLoaded of [false,true]) {
  ctx.ui.setView('settings');
  vm.runInContext(`reposLoaded=${alreadyLoaded};`,ctx);
  document.getElementById('repo').value='owner/previous';
  queryRequests.length=0;
  let failed=false;
  ctx.fetch=(url,options)=>{
    assert(!options?.method);
    if(url.startsWith('/api/repos?')) {
      if(!failed){failed=true;return Promise.resolve(diagnosticResponse({},503));}
      return Promise.resolve(response({current:'owner/previous',repos:[]}));
    }
    if(url.startsWith('/api/search?')) {
      queryRequests.push(new URL(url,'http://127.0.0.1'));
      return Promise.resolve(response({hits:[],vector:'off',why:null}));
    }
    throw new Error('unexpected search retry request');
  };
  await queryButton.listeners.click[0]({target:queryButton});
  assert.equal(queryRequests.length,0,'a failed listing does not issue a scoped query');
  const retry=document.getElementById('status').querySelectorAll('button').at(-1);
  assert(retry,'failed listing offers an explicit retry');
  await retry.listeners.click[0]({target:retry});
  assert.equal(queryRequests.length,1);
  assert.equal(queryRequests[0].searchParams.get('all'),'1','retry retains explicit All');
  assert.equal(queryRequests[0].searchParams.get('repo'),null);
  assert.equal(queryRequests[0].searchParams.get('raw'),'only');
}
console.log('PASS: four tier draft plans, source-backed grouping and unrelated drafts preserved');

}

// W6: the page's address: the saved port beside the one this page is on, a save that moves the
// resident page, a new token, and a foreground run's way to the resident page.
{
const settings=(mode,extra={})=>({version:'v1',first_run:false,resident_supported:true,worker:{resident:true},view:{port:17373},
  summary:{curate:false,language:'Japanese',window_tokens:6000,idle_minutes:10},paid_usd_per_month:0,gemini:null,usd_this_month:0,
  stopped:null,inject:{session_start:true,session_start_note:true,session_start_list:true,per_prompt:false,correction:true,session_start_chars:6000,
    per_prompt_chars:1500,per_prompt_wait_ms:1500,correction_chars:800},capture:{store_prompts:false,tool_output:'full'},backup:{dir:null},
  retention:{days:null},
  redaction:{extra_rules:[],allowlist:[]},chain:[],providers:[],warnings:[],key_input:null,
  ranges:{session_start_chars:[0,20000],per_prompt_chars:[0,5000],per_prompt_wait_ms:[100,3000],correction_chars:[0,2000],window_tokens:[1000,100000],
    idle_minutes:[1,120],paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535],retention_days:[1,36500],my_work_calls:[1,10000]},
  my_work:{curate:false,calls_a_day:100,estimate:null},view_runtime:{port:17373,mode},...extra});
vm.runInContext('globalThis.w6={formOf,saveBody,applySavedSettings,viewDraft:()=>viewDraft,providerOperation};',ctx);
const {w6}=ctx;
const json=(value,status=200)=>({ok:status>=200&&status<300,status,headers:{get:()=>'application/json'},json:async()=>value});
const posted=[];let answers=[];
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){posted.push({url,body:options.body});return Promise.resolve(answers.shift());}
  return new Promise(()=>{});
};
const settle=()=>new Promise(resolve=>setImmediate(resolve));
const section=()=>panel.querySelector('.view-address');
const button=()=>section()?.querySelectorAll('button').find(node=>node.dataset.action?.startsWith('view.'));
const field=name=>panel.querySelectorAll('input').find(node=>node.dataset.field===name);
ctx.ui.setLang('en');ctx.ui.setView('settings');

// The port field shows what is saved; a save sends a port only when it changed.
let f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.equal(field('view.port').value,'17373');
let shown=text(panel);
assert(shown.includes('Saved: 17373')&&shown.includes('moves this page there at once'),'resident port field and its effect');
assert.equal(w6.saveBody().body.view,undefined,'an unchanged port is not sent');
f.view.port='17400';
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.view)),{port:17400});
f.view.port='65536';
assert.equal(w6.saveBody().field,'view.port','an out-of-range port is refused before sending');

// A save the resident viewer answers from another port: the page follows the answer's address,
// which carries the new token, and nothing else.
ctx.location.replaced.length=0;
const moved='http://127.0.0.1:17400/#t=ffeeddccbbaa99887766554433221100';
w6.applySavedSettings(settings('resident',{view:{port:17400},view_runtime:{port:17400,mode:'resident',url:moved}}),f,null);
assert.deepEqual(Array.from(ctx.location.replaced),[`${moved}&moved=1`]);
ctx.location.replaced.length=0;
for(const url of [undefined,'http://example.com:17400/#t=ffeeddccbbaa99887766554433221100']){
  w6.applySavedSettings(settings('resident',{view:{port:17400},view_runtime:{port:17400,mode:'resident',url}}),f,null);
  assert.equal(ctx.location.replaced.length,0,'a move without an address the page can vet is not followed');
  assert(document.getElementById('status').textContent.includes('run oboete view'),'the page says how to find the address');
}
ctx.location.replaced.length=0;
w6.applySavedSettings(settings('foreground',{view:{port:17400},view_runtime:{port:4323,mode:'foreground'}}),f,null);
assert.equal(ctx.location.replaced.length,0,'a foreground run stays on its own address');
// Port 80 is no port in the browser's address: a save that stays there is no move.
ctx.location.port='';
w6.applySavedSettings(settings('resident',{view:{port:80},view_runtime:{port:80,mode:'resident'}}),f,null);
assert.equal(ctx.location.replaced.length,0,'a save on port 80 stays where it is');
assert(!document.getElementById('status').textContent.includes('run oboete view'),'a save on port 80 is not a lost move');
ctx.location.port='17373';

// A port another program holds is refused as such: no stale reload, and what was typed stays.
f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
f.view.port='17400';
answers=[json({code:'port_unavailable',field:'view.port'},409)];
// Decision 69: the port moves the page, so it saves with its own button.
panel.querySelectorAll('button').find(node=>node.dataset.action==='port.move').click();
await settle();
assert(document.getElementById('status').textContent.includes('This port cannot be used'),'the refusal is shown, not a stale save');
assert.equal(f.view.port,'17400','the port typed is kept');

// A new token: confirmed first, then the page goes on only at a vetted address.
f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(section()).includes('old bookmark'),'the effect is explained before the action');
assert(button().disabled,'a new token waits for the confirmation');
const consent=section().querySelectorAll('input').find(node=>node.dataset.field==='view.confirmed');
consent.value=true;consent.checked=true;for(const cb of consent.listeners.change||[])cb({target:consent});
assert(!button().disabled);
answers=[json({url:'http://evil.example:80/#t=0123456789abcdef0123456789abcdef'})];
button().click();await settle();
assert.equal(ctx.location.replaced.length,0,'an address the page cannot vet is not followed');
assert.deepEqual(posted.at(-1),{url:'/api/view/token',body:'{}'});
assert(button().disabled,'the confirmation is asked for again');
w6.viewDraft().confirmed=true;ctx.ui.drawSettings();
const next='http://127.0.0.1:17401/#t=fedcba9876543210fedcba9876543210';
answers=[json({url:next})];
button().click();await settle();
assert.deepEqual(Array.from(ctx.location.replaced),[`${next}&moved=1`]);
ctx.location.replaced.length=0;
w6.viewDraft().confirmed=true;ctx.ui.drawSettings();
answers=[json({code:'unchanged',field:''},503)];
button().click();await settle();
assert(text(section()).includes('The token was not replaced'),'a refusal says nothing changed');
w6.viewDraft().confirmed=true;ctx.ui.drawSettings();
answers=[Promise.reject(new Error('invented lost answer'))];
button().click();await settle();
assert(text(section()).includes('oboete view'),'a lost answer names the way back');

// A foreground run of a home saved resident offers the resident page; one not saved so, nothing.
f=w6.formOf(settings('foreground'));ctx.ui.setForm(f);ctx.ui.drawSettings();
shown=text(panel);
assert(shown.includes('keeps its own address')&&text(section()).includes('port 17373'),'foreground effect and the fixed port');
answers=[json({code:'port_in_use',field:''},503)];
button().click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/view/resident',body:'{}'});
assert(text(section()).includes('Another program uses port 17373'));
answers=[json({url:'http://127.0.0.1:17373/#t=00112233445566778899aabbccddeeff'})];
button().click();await settle();
assert.deepEqual(Array.from(ctx.location.replaced),['http://127.0.0.1:17373/#t=00112233445566778899aabbccddeeff&moved=1']);
f=w6.formOf(settings('foreground',{worker:{resident:false}}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.equal(section(),null,'nothing to start before the resident choice is saved');
ctx.ui.setLang('ja');f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(section()).includes('古いブックマーク')&&text(panel).includes('常駐の画面のポート'),'Japanese page address');
ctx.ui.setLang('en');
console.log('PASS: page address, moves, new token and the resident page from a foreground run');

// docs/retention.md: how long raw records are kept, in the privacy group; forever by default, and
// a period sent only when it changed.
f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('How long raw records are kept')&&text(panel).includes('Saved: forever'),'the retention section, forever');
assert.equal(field('retention.days').disabled,true,'no day count while kept forever');
assert.equal(w6.saveBody().body.retention,undefined,'an unchanged period is not sent');
const tick=(node,v)=>{node.checked=v;for(const cb of node.listeners.change||[])cb({target:node});};
const typeIn=(node,v)=>{node.value=v;for(const cb of node.listeners.input||[])cb({target:node});};
tick(field('retention.forever'),false);
assert.equal(field('retention.days').disabled,false,'unticking Keep forever opens the day count');
typeIn(field('retention.days'),'30');
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.retention)),{days:30});
for(const bad of ['0','36501','1.5','']){f.retention.edit=bad;assert.equal(w6.saveBody().field,'retention.days',`period ${bad} refused before sending`);}
f=w6.formOf(settings('resident',{retention:{days:30}}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('Saved: 30 days'),'a saved period');
assert.equal(field('retention.days').value,'30');
assert.equal(w6.saveBody().body.retention,undefined,'an unchanged period is not sent');
tick(field('retention.forever'),true);
assert.equal(field('retention.days').disabled,true,'ticking Keep forever closes it');
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.retention)),{days:null},'forever is sent as null');
f=w6.formOf(settings('resident',{retention:{days:1}}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('Saved: 1 day')&&!text(panel).includes('Saved: 1 days'),'one day');
f=w6.formOf(settings('resident',{retention:{days:30}}));
ctx.ui.setLang('ja');ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('元の記録を残す期間')&&text(panel).includes('保存済み: 30 日'),'Japanese retention section');
ctx.ui.setLang('en');
console.log('PASS: the retention period');
// Decision 69 (69b-1): the detailed page saves each change at once, with no Save bar. Nothing
// unchanged is sent; an invalid value is marked and not sent; the port keeps its button; no
// monthly cap, masking off and a saved masking rule removed wait for their confirmation; a new
// masking rule waits until it has its id and pattern, and survives the saves meanwhile.
{
  // The event reaches the control's own form, as it bubbles there: a control outside the general
  // forms reaches none (decision 69, 69b-2).
  const fire=(name,ev)=>{for(const cb of ev.target?.closest?.('form')?.listeners[name]||[])cb(ev);};
  const sent=()=>posted.map(p=>JSON.parse(p.body));
  const any=name=>panel.querySelectorAll('input, textarea').find(node=>node.dataset.field===name);
  const action=name=>[...panel.querySelectorAll('button'),...document.getElementById('status').querySelectorAll('button')].filter(n=>n.dataset.action===name).at(-1);
  // Leaving a field moves the focus where it went, as the browser does.
  const leave=(node,next=null)=>{fire('change',{target:node});fire('focusout',{target:node,relatedTarget:next});if(next)next.focus();else document.activeElement=null;};
  // markInvalid reads the document's controls: here, the page's.
  const pageControls=document.querySelectorAll;document.querySelectorAll=sel=>panel.querySelectorAll(sel);
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  assert(!panel.querySelector('.save-bar'),'no Save bar');
  answers=[json(settings('resident',{version:'v2',worker:{resident:false}}))];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});await settle();
  assert.equal(posted.length,1,'a switch saves when it changes');
  assert.equal(sent()[0].worker.resident,false);assert.equal(sent()[0].view,undefined,'and sends no port');
  fire('change',{target:field('worker.resident')});await settle();
  assert.equal(posted.length,1,'an unchanged form sends nothing');
  const inject={...settings('resident').inject,session_start_chars:5000};
  answers=[json(settings('resident',{version:'v3',worker:{resident:false},inject}))];
  typeIn(field('inject.session_start_chars'),'5000');fire('change',{target:field('inject.session_start_chars')});
  assert.equal(posted.length,1,'a number is not sent while focus is still on it');
  fire('focusout',{target:field('inject.session_start_chars'),relatedTarget:field('inject.per_prompt_chars')});field('inject.per_prompt_chars').focus();await settle();
  assert.equal(posted.length,2,'it is sent when left');assert.equal(sent()[1].inject.session_start_chars,5000);
  assert.equal(document.activeElement?.dataset.field,'inject.per_prompt_chars','the focus goes back where it went');
  // A later redraw (privacy read again after the save) keeps it there.
  const before=document.activeElement;ctx.ui.drawSettings();
  assert(document.activeElement!==before&&document.activeElement===field('inject.per_prompt_chars'),'a redraw keeps the focus on its field');
  // Typing goes on while a save is on its way: the page stays usable, and what was typed then
  // survives the save's answer and is saved when its field is left.
  let answer;answers=[new Promise(resolve=>{answer=resolve;})];
  typeIn(field('inject.session_start_chars'),'4500');leave(field('inject.session_start_chars'),field('inject.per_prompt_chars'));await settle();
  assert.equal(panel.querySelector('.settings').inert,false,'the page is not frozen by a save on its way');
  typeIn(field('inject.per_prompt_chars'),'1200');
  answer(json(settings('resident',{version:'v4',worker:{resident:false},inject:{...inject,session_start_chars:4500}})));await settle();await settle();
  assert.equal(field('inject.per_prompt_chars').value,'1200','what was typed during the save stays');
  assert.equal(field('inject.session_start_chars').value,'4500','the saved value shows');
  answers=[json(settings('resident',{version:'v5',worker:{resident:false},inject:{...inject,session_start_chars:4500,per_prompt_chars:1200}}))];
  leave(field('inject.per_prompt_chars'));await settle();
  assert.equal(sent().at(-1).inject.per_prompt_chars,1200,'and is saved when left');
  assert.equal(sent().at(-1).version,'v4','with the version the save answered');
  typeIn(field('inject.session_start_chars'),'99999');leave(field('inject.session_start_chars'));await settle();
  assert.equal(posted.length,4,'an out-of-range number is not sent');
  assert.equal(field('inject.session_start_chars').attrs['aria-invalid'],'true','it is marked');
  assert(document.getElementById('status').textContent.includes('out of range'),'and said');
  // The port: leaving it sends nothing; its button saves it.
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  typeIn(field('view.port'),'17400');leave(field('view.port'));await settle();
  assert.equal(posted.length,0,'a port typed is not saved by leaving it');
  answers=[json({code:'port_unavailable',field:'view.port'},409)];
  action('port.move').click();await settle();
  assert.equal(sent()[0].view.port,17400,'its button saves it');
  // No monthly cap and masking off are the Privacy and AI groups' own switches, each with its
  // confirmation (decision 69, 69b-2). A draft of either that a general save would carry (the
  // guide's Apply can bring one) is asked first, saved once confirmed.
  const nudge=()=>fire('change',{target:field('worker.resident')});
  f=w6.formOf(settings('resident',{paid_usd_per_month:5}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  f.paidNoCap=true;nudge();await settle();
  assert.equal(posted.length,0,'no cap is not saved at once');
  assert(document.getElementById('status').textContent.includes('does not stop paid calls'),'its warning asks first');
  answers=[json(settings('resident',{version:'v2',paid_usd_per_month:null}))];
  action('held.confirm').focus();action('held.confirm').click();await settle();
  assert.equal(posted.length,1);assert.equal(sent()[0].paid_usd_per_month,null,'confirmed, it is saved');
  assert.equal(document.activeElement?.dataset.field,'worker.resident','the focus goes back to the control changed');
  // Masking off: asked first; Cancel goes back to what is saved.
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  f.redaction.enabled=false;nudge();await settle();
  assert.equal(posted.length,0,'masking off is not saved at once');
  action('held.cancel').click();
  assert.equal(ctx.ui.getForm().redaction.enabled,true,'cancelled, the form goes back to what is saved');
  // A new masking rule waits until it has its id and pattern, and survives a save meanwhile.
  const rule={id:'acme-token',regex:'acme_[a-z0-9]{8}',keywords:[],entropy:null,secret_group:null};
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[rule],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  const add=panel.querySelectorAll('button').find(n=>text(n)==='Add a masking rule');
  add.click();fire('click',{target:add});await settle();
  assert.equal(posted.length,0,'an empty new rule sends nothing');
  typeIn(any('redaction.extra_rules.1.id'),'beta-key');leave(any('redaction.extra_rules.1.id'));await settle();
  assert.equal(posted.length,0,'half a rule sends nothing, and no error');
  answers=[json(settings('resident',{version:'v2',worker:{resident:false},redaction:{enabled:true,extra_rules:[rule],allowlist:[]}}))];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});await settle();
  assert.equal(sent()[0].redaction.extra_rules.length,1,'another change saves without the unfinished rule');
  assert.equal(any('redaction.extra_rules.1.id')?.value,'beta-key','which stays to be finished');
  const beta={id:'beta-key',regex:'beta_[0-9]+',keywords:[],entropy:null,secret_group:null};
  answers=[json(settings('resident',{version:'v3',worker:{resident:false},redaction:{enabled:true,extra_rules:[rule,beta],allowlist:[]}}))];
  typeIn(any('redaction.extra_rules.1.regex'),'beta_[0-9]+');leave(any('redaction.extra_rules.1.regex'));await settle();
  assert.deepEqual(sent()[1].redaction.extra_rules.map(r=>r.id),['acme-token','beta-key'],'finished, it is sent');
  // A saved rule removed is asked first; an emptied saved rule is refused, never dropped.
  posted.length=0;
  typeIn(any('redaction.extra_rules.0.id'),'');leave(any('redaction.extra_rules.0.id'));await settle();
  assert.equal(posted.length,0,'an emptied saved rule is not sent');
  assert.equal(any('redaction.extra_rules.0.id').attrs['aria-invalid'],'true','it is marked');
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[rule],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  const remove=panel.querySelectorAll('button').find(n=>text(n)==='Remove this rule');
  remove.click();fire('click',{target:remove});await settle();
  assert.equal(posted.length,0,'a saved rule removed waits');
  assert(document.getElementById('status').textContent.includes('acme-token'),'it names the rule');
  answers=[json(settings('resident',{version:'v2',redaction:{enabled:true,extra_rules:[],allowlist:[]}}))];
  action('held.confirm').click();await settle();
  assert.equal(sent()[0].redaction.extra_rules.length,0,'confirmed, it is removed');
  // Round 1 of #802. Saves go one at a time: a change made while one is on its way is sent after
  // its answer, with that answer's version (Codex, cubic and CodeRabbit).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let first;answers=[new Promise(resolve=>{first=resolve;})];
  const noteOff={...settings('resident').inject,session_start_note:false};
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  tick(field('inject.session_start_note'),false);fire('change',{target:field('inject.session_start_note')});await settle();
  assert.equal(posted.length,1,'a change made while a save is on its way waits for its answer');
  answers=[json(settings('resident',{version:'v3',worker:{resident:false},inject:noteOff}))];
  first(json(settings('resident',{version:'v2',worker:{resident:false}})));await settle();await settle();
  assert.equal(posted.length,2,'and is sent after it');
  assert.equal(sent()[1].version,'v2','with the version that answer gave');
  assert.equal(sent()[1].inject.session_start_note,false);assert.equal(sent()[1].worker.resident,false);
  // A confirm left in the status line waits behind a save on its way as well (round 1's review).
  f=w6.formOf(settings('resident',{paid_usd_per_month:5}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  f.paidNoCap=true;nudge();await settle();
  const staleConfirm=action('held.confirm');
  f.paidNoCap=false;nudge();await settle();
  let held;answers=[new Promise(resolve=>{held=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});await settle();
  assert.equal(posted.length,1);
  staleConfirm.click();await settle();
  assert.equal(posted.length,1,'a confirm clicked while a save is on its way sends nothing beside it');
  held(json(settings('resident',{version:'v2',paid_usd_per_month:5,worker:{resident:false}})));await settle();await settle();
  assert.equal(posted.length,1,'and after its answer, nothing is left to send');
  // A port typed and not yet moved to stays typed through another change's save (Codex, cubic).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  typeIn(field('view.port'),'17400');leave(field('view.port'));await settle();
  answers=[json(settings('resident',{version:'v2',worker:{resident:false}}))];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});await settle();
  assert.equal(sent()[0].view,undefined,'another change sends no port');
  assert.equal(field('view.port').value,'17400','and the port typed stays');
  // The port's button asks first for what waits for a confirmation, as any save does (Codex, cubic
  // and CodeRabbit), and every change waiting is said and confirmed together (CodeRabbit).
  f=w6.formOf(settings('resident',{paid_usd_per_month:5,redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  f.paidNoCap=true;f.redaction.enabled=false;
  typeIn(field('view.port'),'17400');
  action('port.move').click();await settle();
  assert.equal(posted.length,0,'the port button does not pass a change waiting for its confirmation');
  const asked=document.getElementById('status').textContent;
  assert(asked.includes('does not stop paid calls')&&asked.includes('With masking off'),'both changes waiting are said');
  assert.equal(text(action('held.confirm')),'Confirm and save','and confirmed together');
  answers=[json({code:'port_unavailable',field:'view.port'},409)];
  action('held.confirm').click();await settle();
  assert.equal(posted.length,1);
  assert.equal(sent()[0].view.port,17400);assert.equal(sent()[0].paid_usd_per_month,null);
  assert.equal(sent()[0].redaction.enabled,false,'confirmed, all are sent');
  // The port's button keeps an unfinished new masking rule through its answer, as the other saves
  // do (cubic).
  f=w6.formOf(settings('foreground',{redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  const addDraft=panel.querySelectorAll('button').find(n=>text(n)==='Add a masking rule');
  addDraft.click();fire('click',{target:addDraft});await settle();
  typeIn(any('redaction.extra_rules.0.id'),'delta-key');leave(any('redaction.extra_rules.0.id'));await settle();
  typeIn(field('view.port'),'17400');
  answers=[json(settings('foreground',{version:'v2',view:{port:17400},view_runtime:{port:4323,mode:'foreground'},redaction:{enabled:true,extra_rules:[],allowlist:[]}}))];
  action('port.move').click();await settle();await settle();
  assert.equal(posted.length,1);assert.equal(sent()[0].view.port,17400);
  assert.equal(any('redaction.extra_rules.0.id')?.value,'delta-key','an unfinished rule survives the port\'s save');
  // A new rule saved while something else was typed is a saved rule from then on: emptied, it is
  // refused, never dropped (cubic).
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  const addRule=panel.querySelectorAll('button').find(n=>text(n)==='Add a masking rule');
  addRule.click();fire('click',{target:addRule});await settle();
  typeIn(any('redaction.extra_rules.0.id'),'gamma-key');leave(any('redaction.extra_rules.0.id'));await settle();
  let ruleAnswer;answers=[new Promise(resolve=>{ruleAnswer=resolve;})];
  typeIn(any('redaction.extra_rules.0.regex'),'gamma_[0-9]+');leave(any('redaction.extra_rules.0.regex'));await settle();
  assert.equal(posted.length,1,'finished, the rule is sent');
  typeIn(field('inject.session_start_chars'),'4000');
  const gamma={id:'gamma-key',regex:'gamma_[0-9]+',keywords:[],entropy:null,secret_group:null};
  ruleAnswer(json(settings('resident',{version:'v2',redaction:{enabled:true,extra_rules:[gamma],allowlist:[]}})));await settle();await settle();
  assert.equal(field('inject.session_start_chars').value,'4000','what was typed meanwhile stays');
  typeIn(any('redaction.extra_rules.0.id'),'');leave(any('redaction.extra_rules.0.id'));await settle();
  assert.equal(posted.length,1,'the saved rule emptied is not sent');
  assert.equal(any('redaction.extra_rules.0.id').attrs['aria-invalid'],'true','it is refused as a saved rule');
  // A field in a fold the save's redraw closes gets the focus by opening the fold (CodeRabbit).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;document.activeElement=null;
  answers=[json(settings('resident',{version:'v2',summary:{...settings('resident').summary,window_tokens:7000}}))];
  typeIn(field('summary.window_tokens'),'7000');leave(field('summary.window_tokens'),field('summary.idle_minutes'));await settle();
  assert.equal(posted.length,1);
  assert.equal(document.activeElement,field('summary.idle_minutes'),'the focus goes to the field it went to');
  assert.equal(field('summary.idle_minutes').parentElement.closest('details').open,true,'its fold opened');
  // Enter saves a field; on a link or a button it keeps its own meaning (CodeRabbit).
  let stopped=false;
  const link=document.createElement('a');field('inject.session_start_chars').closest('form').append(link);
  fire('keydown',{key:'Enter',target:link,preventDefault(){stopped=true;}});
  assert.equal(stopped,false,'Enter on a link is left alone');
  fire('keydown',{key:'Enter',target:field('inject.session_start_chars'),preventDefault(){stopped=true;}});
  assert.equal(stopped,true,'Enter in a field is the field\'s');
  // The guide points at no Save button: there is none (Codex).
  assert.equal(panel.querySelectorAll('button').filter(n=>!n.hidden&&text(n)==='Save settings').length,0,'nothing points at a Save button');
  // A hash is saved once it is made: the click's own save comes before it (Codex and cubic).
  ctx.crypto.subtle=globalThis.crypto.subtle;
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  const exact=panel.querySelectorAll('input').find(n=>n.type==='password');
  const hashButton=panel.querySelectorAll('button').find(n=>text(n)==='Add its hash');
  typeIn(exact,'a made-up secret');
  answers=[json(settings('resident',{version:'v2'}))];
  hashButton.click();fire('click',{target:hashButton});
  for(let i=0;i<50&&!posted.length;i++)await new Promise(resolve=>setTimeout(resolve,10));
  assert.equal(posted.length,1,'the hash made is saved');
  const made=(await globalThis.crypto.subtle.digest('SHA-256',new TextEncoder().encode('a made-up secret')));
  assert.deepEqual(sent()[0].redaction.allowlist,[Buffer.from(made).toString('hex')],'with the hash of what was typed');
  delete ctx.crypto.subtle;await settle();
  // Round 2 of #802. A value sent only when it changed is compared with what the last answer
  // saved: a period saved while another field was typed goes back to forever (Codex).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  tick(field('retention.forever'),false);fire('change',{target:field('retention.forever')});await settle();
  assert.equal(posted.length,0,'a period not yet typed is not sent');
  let periodAnswer;answers=[new Promise(resolve=>{periodAnswer=resolve;})];
  typeIn(field('retention.days'),'30');leave(field('retention.days'),field('inject.session_start_chars'));await settle();
  assert.deepEqual(sent()[0].retention,{days:30});
  typeIn(field('inject.session_start_chars'),'4000');
  periodAnswer(json(settings('resident',{version:'v2',retention:{days:30}})));await settle();await settle();
  answers=[json(settings('resident',{version:'v3',inject:{...settings('resident').inject,session_start_chars:4000}}))];
  tick(field('retention.forever'),true);fire('change',{target:field('retention.forever')});await settle();
  assert.deepEqual(sent().at(-1).retention,{days:null},'forever again is sent');
  // ...and a backup folder saved so goes back to the default when emptied (Codex).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let folderAnswer;answers=[new Promise(resolve=>{folderAnswer=resolve;})];
  typeIn(field('backup.dir'),'/srv/made-up-backups');leave(field('backup.dir'),field('inject.session_start_chars'));await settle();
  assert.deepEqual(sent()[0].backup,{dir:'/srv/made-up-backups'});
  typeIn(field('inject.session_start_chars'),'4000');
  folderAnswer(json(settings('resident',{version:'v2',backup:{dir:'/srv/made-up-backups'}})));await settle();await settle();
  answers=[json(settings('resident',{version:'v3',inject:{...settings('resident').inject,session_start_chars:4000}}))];
  typeIn(field('backup.dir'),'');leave(field('backup.dir'));await settle();
  assert.deepEqual(sent().at(-1).backup,{dir:null},'emptied, the folder goes back to the default');
  // ...and the judge's destination chosen back after one saved so is sent (Codex).
  const judgeDests=[['jev',0.042],['clef',0.24]].map(([id,usd])=>({id,key:'ok',usd_per_mtok_in:usd,local:false}));
  const judgeAt=destination=>({set:true,enabled:true,destination,destinations:judgeDests,key:'ok',key_file:'/home/made-up/.local/share/oboete/keys/k1',
    cloudflare_account_id:null,daily_budget:300,calls_today:0,usd_this_month:0});
  const to=()=>panel.querySelectorAll('select').find(n=>n.dataset.field==='judge.destination');
  const choose=v=>{to().value=v;for(const cb of to().listeners.change||[])cb({target:to()});fire('change',{target:to()});};
  f=w6.formOf(settings('resident',{judge:judgeAt('jev')}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let toAnswer;answers=[new Promise(resolve=>{toAnswer=resolve;})];
  choose('clef');
  assert.equal(sent()[0].judge.destination,'clef');
  typeIn(field('inject.session_start_chars'),'4000');
  toAnswer(json(settings('resident',{version:'v2',judge:judgeAt('clef')})));await settle();await settle();
  answers=[json(settings('resident',{version:'v3',judge:judgeAt('jev')}))];
  choose('jev');await settle();
  assert.equal(sent().at(-1).judge?.destination,'jev','the destination chosen back is sent');
  // ...and search's Cloudflare account typed back after another saved so is sent (Codex).
  const searchAt=account=>({provider:'none',workers_ai:{account_id:account,key_file:'/home/made-up/CF_WORKERS_AI_KEY.md',key:'missing',
    daily_requests:200,monthly_usd:1,usd_this_month:0,requests_today:0},local:{unavailable:null,files:'not_downloaded',dir:'/home/made-up/.oboete/models/bge-m3',held:0}});
  const [first32,second32]=['a','b'].map(c=>c.repeat(32));
  const searchRanges={...settings('resident').ranges,daily_requests:[1,1000000]};
  f=w6.formOf(settings('resident',{embedding:searchAt(first32),ranges:searchRanges}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let accountAnswer;answers=[new Promise(resolve=>{accountAnswer=resolve;})];
  typeIn(field('embedding.account_id'),second32);leave(field('embedding.account_id'),field('inject.session_start_chars'));await settle();
  assert.equal(sent()[0].embedding.account_id,second32);
  typeIn(field('inject.session_start_chars'),'4000');
  accountAnswer(json(settings('resident',{version:'v2',embedding:searchAt(second32),ranges:searchRanges})));await settle();await settle();
  answers=[json(settings('resident',{version:'v3',embedding:searchAt(first32),ranges:searchRanges}))];
  typeIn(field('embedding.account_id'),first32);leave(field('embedding.account_id'));await settle();
  assert.equal(sent().at(-1).embedding?.account_id,first32,'the account typed back is sent');
  // A masking rule added and typed while a save is on its way stays (Codex), as do a place not yet
  // added to those never recorded and the guide's phrase (CodeRabbit).
  f=w6.formOf(settings('resident',{redaction:{enabled:true,extra_rules:[],allowlist:[]}}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let ruleLater;answers=[new Promise(resolve=>{ruleLater=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  const addLater=panel.querySelectorAll('button').find(n=>text(n)==='Add a masking rule');
  addLater.click();fire('click',{target:addLater});
  typeIn(any('redaction.extra_rules.0.id'),'epsilon-key');
  Object.assign(f,{neverFolder:'/home/made-up/private',neverPattern:'made-up-*',testPhrase:'a made-up phrase'});
  ruleLater(json(settings('resident',{version:'v2',worker:{resident:false},redaction:{enabled:true,extra_rules:[],allowlist:[]}})));await settle();await settle();
  assert.notEqual(ctx.ui.getForm(),f,'the answer is the form now');
  assert.equal(any('redaction.extra_rules.0.id')?.value,'epsilon-key','the rule added during the save stays');
  const kept=ctx.ui.getForm();
  assert.deepEqual([kept.neverFolder,kept.neverPattern,kept.testPhrase],['/home/made-up/private','made-up-*','a made-up phrase'],'and what no body carries');
  assert.equal(posted.length,1,'and nothing more is sent for them');
  // The focus moved on while a save was on its way stays where it was moved (Codex).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let moveAnswer;answers=[new Promise(resolve=>{moveAnswer=resolve;})];
  typeIn(field('inject.session_start_chars'),'5000');leave(field('inject.session_start_chars'),field('inject.per_prompt_chars'));await settle();
  field('inject.correction_chars').focus();
  moveAnswer(json(settings('resident',{version:'v2',inject:{...settings('resident').inject,session_start_chars:5000}})));await settle();await settle();
  assert.equal(document.activeElement?.dataset.field,'inject.correction_chars','the focus stays where the owner moved it');
  // The texts point at no Save button (CodeRabbit).
  for(const key of ['onboarding_desc','provider_entries_desc'])
    assert(!ctx.tier.text(key,'en').includes('Save settings')&&!ctx.tier.text(key,'ja').includes('「設定を保存」'),`${key} names no Save button`);
  ctx.ui.setLang('en');
  // Enter in the port's field, with a change waiting for its confirmation, moves the port once
  // confirmed: the form's own save after it does not take the confirmation's place (CodeRabbit).
  f=w6.formOf(settings('resident',{paid_usd_per_month:5}));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  f.paidNoCap=true;nudge();await settle();
  typeIn(field('view.port'),'17400');
  const enter={key:'Enter',target:field('view.port'),preventDefault(){}};
  for(const cb of field('view.port').listeners.keydown||[])cb(enter);
  fire('click',{target:action('port.move')});fire('keydown',enter);await settle();
  assert.equal(posted.length,0);
  // Going to the confirm leaves the port's field: that saves nothing, and its confirmation stays.
  const portAsk=action('held.confirm');
  leave(field('view.port'),portAsk);await settle();
  assert.equal(posted.length,0);assert.equal(action('held.confirm'),portAsk,'the port\'s confirmation stays');
  answers=[json({code:'port_unavailable',field:'view.port'},409)];
  action('held.confirm').click();await settle();
  assert.equal(sent().at(-1)?.view?.port,17400,'confirmed, the port is sent');
  // The Enter that ends an input method's composition is the input method's (CodeRabbit).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  typeIn(field('backup.dir'),'/srv/バックアップ');
  for(const composing of [{isComposing:true},{keyCode:229}]){
    let taken=false;fire('keydown',{key:'Enter',target:field('backup.dir'),preventDefault(){taken=true;},...composing});
    assert.equal(taken,false,'the composition\'s Enter is left alone');
  }
  await settle();
  assert.equal(posted.length,0,'and saves nothing');
  // Round 3 of #802. A field the answer of a save drew again while it was typed in is saved when
  // left, with no change of its own after the redraw (Codex).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let redrawn;answers=[new Promise(resolve=>{redrawn=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  field('inject.per_prompt_chars').focus();typeIn(field('inject.per_prompt_chars'),'1200');fire('input',{target:field('inject.per_prompt_chars')});
  redrawn(json(settings('resident',{version:'v2',worker:{resident:false}})));await settle();await settle();
  assert.equal(field('inject.per_prompt_chars').value,'1200','what was typed is drawn again');
  answers=[json(settings('resident',{version:'v3',worker:{resident:false},inject:{...settings('resident').inject,per_prompt_chars:1200}}))];
  fire('focusout',{target:field('inject.per_prompt_chars'),relatedTarget:null});await settle();
  assert.equal(sent().at(-1).inject?.per_prompt_chars,1200,'left with no change after the redraw, it is saved');
  // A provider's write waits for a save on its way and sends that answer's version; a change made
  // while the write is on its way waits for it and is saved after it (Codex).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let saving;answers=[new Promise(resolve=>{saving=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  const write=w6.providerOperation({op:'enabled',selector:{source:'config',name:'made-up'},enabled:false},action('port.move'));
  await settle();
  assert.equal(posted.length,1,'a provider write waits for the save on its way');
  let writing;answers=[new Promise(resolve=>{writing=resolve;})];
  saving(json(settings('resident',{version:'v2',worker:{resident:false}})));await settle();await settle();
  assert.equal(posted.length,2);assert.equal(posted[1].url,'/api/providers');
  assert.equal(JSON.parse(posted[1].body).version,'v2','and sends with the version that save gave');
  tick(field('inject.session_start_note'),false);fire('change',{target:field('inject.session_start_note')});await settle();
  assert.equal(posted.length,2,'a change made while the provider write is on its way waits for it');
  answers=[json(settings('resident',{version:'v4',worker:{resident:false},inject:{...settings('resident').inject,session_start_note:false}}))];
  writing(json(settings('resident',{version:'v3',worker:{resident:false}})));await write;await settle();
  assert.equal(posted.length,3);assert.equal(sent().at(-1).inject?.session_start_note,false);
  assert.equal(sent().at(-1).version,'v3','and is saved after it, with the version it gave');
  // Round 4 of #802. Waiting for a save on its way, the provider write's control and the page wait
  // too, so no other write is chosen meanwhile; the page the save drew again waits for the write
  // (Codex, cubic and CodeRabbit).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let waitSave;answers=[new Promise(resolve=>{waitSave=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  const control=action('port.move');
  const queued=w6.providerOperation({op:'enabled',selector:{source:'config',name:'made-up'},enabled:false},control);
  await settle();
  assert.equal(control.disabled,true,'the control waits disabled');
  assert.equal(panel.querySelector('.settings').inert,true,'and the page with it');
  let writeDone;answers=[new Promise(resolve=>{writeDone=resolve;})];
  waitSave(json(settings('resident',{version:'v2',worker:{resident:false}})));await settle();await settle();
  assert.equal(posted.length,2);
  assert.equal(panel.querySelector('.settings').inert,true,'the page the save drew again waits for the write');
  writeDone(json(settings('resident',{version:'v3',worker:{resident:false}})));await queued;await settle();
  assert(!panel.querySelector('.settings').inert,'and is usable after it');
  // A write that waited while the AI entries changed sends nothing, and says so (Codex and cubic).
  const entry=(name,index)=>({name,selector:{source:'config',index},saved:{enabled:true,kind:'openai',base_url:'http://127.0.0.1:9/v1',
    model:'made-up',key:'none',timeout_s:60,limits:{}},effective:{on:true,order:index,model:'made-up',timeout_s:60}});
  const listed=(names,extra={})=>settings('resident',{providers:names.map((name,index)=>entry(name,index)),...extra});
  f=w6.formOf(listed(['made-up-a','made-up-b']));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let reordered;answers=[new Promise(resolve=>{reordered=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  const stale=w6.providerOperation({op:'enabled',selector:{source:'config',index:0},enabled:false},action('port.move'));
  await settle();
  reordered(json(listed(['made-up-b','made-up-a'],{version:'v2',worker:{resident:false}})));await stale;await settle();
  assert.equal(posted.length,1,'a write that waited while the list changed is not sent');
  assert(document.getElementById('status').textContent.includes('changed while this waited'),'and the page says so');
  assert(!panel.querySelector('.settings').inert,'and is usable');
  // ...nor one whose entries changed places with the same names, as a write moves an entry from
  // the built-in list into the file.
  const filed=(names,extra={})=>settings('resident',{providers:names.map((name,index)=>({...entry(name,index),selector:{source:'file',index}})),...extra});
  f=w6.formOf(listed(['made-up-a','made-up-b']));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  let replaced;answers=[new Promise(resolve=>{replaced=resolve;})];
  tick(field('worker.resident'),false);fire('change',{target:field('worker.resident')});
  const restaged=w6.providerOperation({op:'enabled',selector:{source:'config',index:0},enabled:false},action('port.move'));
  await settle();
  replaced(json(filed(['made-up-a','made-up-b'],{version:'v2',worker:{resident:false}})));await restaged;await settle();
  assert.equal(posted.length,1,'nor one whose entries changed places');
  // The port's Enter that ends an input method's composition moves nothing (CodeRabbit).
  f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();posted.length=0;
  typeIn(field('view.port'),'17400');
  for(const composing of [{isComposing:true},{keyCode:229}]){
    let taken=false;const ev={key:'Enter',target:field('view.port'),preventDefault(){taken=true;},...composing};
    for(const cb of field('view.port').listeners.keydown||[])cb(ev);
    assert.equal(taken,false,'the port\'s composition Enter is left alone');
  }
  await settle();
  assert.equal(posted.length,0,'and moves nothing');
  document.querySelectorAll=pageControls;
  console.log('PASS: decision 69 (69b-1): each change saves at once, the port by its button, three changes asked first');
}
// Decision 51: the memory list switch, below the terminal line's, follows it and is sent.
f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.equal(field('inject.session_start_list').checked,true,'the list is on by default');
assert.equal(field('inject.session_start_list').disabled,false);
assert(text(panel).includes('Also show the list of memories handed over'),'the list switch');
tick(field('inject.session_start_note'),false);
assert.equal(field('inject.session_start_list').disabled,true,'no list without the line');
tick(field('inject.session_start_note'),true);
assert.equal(field('inject.session_start_list').disabled,false);
tick(field('inject.session_start_list'),false);
assert.equal(w6.saveBody().body.inject.session_start_list,false,'the list choice is sent');
ctx.ui.setLang('ja');ctx.ui.drawSettings();
assert(text(panel).includes('渡した記憶の一覧も表示する'),'Japanese list switch');
ctx.ui.setLang('en');
console.log('PASS: the session start list');
// Decision 57 slice 2c: My Work's switch and daily bound, sent as saved or typed, with the estimate
// of what turning it on costs.
f=w6.formOf(settings('resident'));ctx.ui.setForm(f);ctx.ui.drawSettings();
shown=text(panel);
assert(shown.includes('My Work, written by AI')&&shown.includes('An estimate appears here once something is recorded.'),'My Work before anything is recorded');
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.my_work)),{curate:false,calls_a_day:100},'the saved switch and bound are sent');
// My Work on is the group's own switch (decision 69, 69b-2): a general save sends it as saved.
assert(!field('my_work.curate'),'one My Work switch on the page');
typeIn(field('my_work.calls_a_day'),'250');
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.my_work)),{curate:false,calls_a_day:250},'what is typed is sent, the switch as saved');
for(const bad of ['0','10001','1.5','']){f.my_work.calls_a_day=bad;assert.equal(w6.saveBody().field,'my_work.calls_a_day',`bound ${bad} refused before sending`);}
// #680: a bound the file holds out of range is sent as it is while it is not changed.
f=w6.formOf(settings('resident',{my_work:{curate:false,calls_a_day:0,estimate:null}}));ctx.ui.setForm(f);
assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody().body.my_work)),{curate:false,calls_a_day:0},'an unchanged bound out of range is sent as it is');
f.my_work.calls_a_day='10001';assert.equal(w6.saveBody().field,'my_work.calls_a_day','a changed one out of range is refused');
f=w6.formOf(settings('resident',{my_work:{curate:true,calls_a_day:250,estimate:{calls:[3,250],usd:null,priced:null}}}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('Calls a day with it on: about 3, at most 250. No paid AI is in the list'),'the estimate with no paid entry');
f=w6.formOf(settings('resident',{my_work:{curate:true,calls_a_day:250,estimate:{calls:[3,250],usd:[0.0123,6.62],priced:{name:'p',model:'m'}}}}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('At p (m) prices: about $0.012 a day, at most $6.6.'),'the estimate at the first paid entry\'s prices');
ctx.ui.setLang('ja');ctx.ui.drawSettings();
assert(text(panel).includes('オンのときの 1 日の呼び出し: およそ 3 回、多くても 250 回。')&&text(panel).includes('p(m)の料金で'),'the Japanese estimate');
ctx.ui.setLang('en');
console.log('PASS: My Work\'s switch, bound and estimate');
// Decision 64: each cap's No cap switch sets its number aside with a warning, a cap loaded off
// shows as off, and a save carries null for the month and 4294967295 for a day.
{
const NO=4294967295;
const ranges={...settings('resident').ranges,no_cap:NO};
const row=(name,extra={})=>({name,on:true,key:'ok',key_file:null,effective_model:'m',model_rule:'any',model:null,model_applied:true,
  daily_budget:null,timeout_s:null,effective_timeout_s:90,differs:[],entries:1,subscription:false,budget_from_key:false,effective_daily_budget:200,...extra});
const off=name=>field(`${name}.no_cap`);
const warned=()=>panel.querySelectorAll('.no-cap-warning').filter(node=>!node.hidden).length;
f=w6.formOf(settings('resident',{ranges,paid_usd_per_month:5,
  chain:[row('groq'),row('claude',{effective_daily_budget:null}),row('pro',{subscription:true,effective_daily_budget:null})]}));
ctx.ui.setForm(f);ctx.ui.drawSettings();
// The month's No cap is the AI group's own switch, with its confirmation (decision 69, 69b-2; the
// simple page's tests): a general save sends the month as saved.
assert.deepEqual([off('paid_usd_per_month'),warned()],[undefined,1],'a cap shows as a cap; an entry with no calls a day says so');
assert.equal(w6.saveBody().body.paid_usd_per_month,5,'the month is sent as saved');
tick(off('my_work'),true);
assert.deepEqual([field('my_work.calls_a_day').disabled,warned()],[true,2],'No cap sets the number aside and warns');
assert.equal(w6.saveBody().body.my_work.calls_a_day,NO,'a day\'s no cap is sent as 4294967295');
// Each AI's calls a day is its line's (69b-3c): its No cap switch, no table. A general save sends
// `[chain]` as saved, whatever the rows show.
const lineOff=name=>panel.querySelectorAll('input').find(n=>n.dataset.simpleFocus===`budget-off:${name}`);
const daily=()=>JSON.parse(JSON.stringify(w6.saveBody().body.chain.map(r=>r.daily_budget)));
assert.deepEqual([lineOff('groq').checked,lineOff('claude').checked,lineOff('pro'),panel.querySelector('.chain')],[false,true,undefined,null],
  'an entry with no calls a day shows as off; a subscription with none capped has no switch; no chain table');
assert.deepEqual(daily(),[null,null,null],'the rows are sent as config.toml has them');
assert(lineOff('groq').attrs['aria-label'].startsWith('No cap: Calls a day: '),'the switch names its AI');
// Codex on #772 and #773: no control edits the rows in the form, so a general save after any
// answer sends them as that answer saved them, and none turns an entry's own calls a day into a
// [chain] value it did not ask for.
vm.runInContext('globalThis.w6merge=mergeProviderSettings;',ctx);
const edited={op:'edit',selector:{source:'file',index:0},entry:{kind:'openai',subscription:false}};
const loaded=extra=>w6.formOf(settings('resident',{ranges,chain:[row('p',extra)]}));
// An entry with no calls a day of its own, given 7 in its editor: the answer's 7 shows on its line.
f=ctx.w6merge(settings('resident',{ranges,chain:[row('p',{effective_daily_budget:7})]}),loaded({effective_daily_budget:null}),edited);
ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([lineOff('p').checked,field('chain.p.daily_budget').disabled,field('chain.p.daily_budget').placeholder,daily()],[false,false,'7',[null]],
  'a cap set in the entry\'s editor shows on its line, and the next whole save sends [chain] as saved');
// Set back to no cap in its editor, the line shows it so again (#773).
f=ctx.w6merge(settings('resident',{ranges,chain:[row('p',{effective_daily_budget:null})]}),f,edited);ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([lineOff('p').checked,daily()],[true,[null]],'the next answer is followed again');
// Two entries of one name, reordered: each answer's rows are sent as they come.
const moved={op:'move',selector:{source:'file',index:0},to:1};
const two=(effective,daily_budget=null)=>settings('resident',{ranges,chain:[row('p',{entries:2,differs:['daily_budget'],effective_daily_budget:effective,daily_budget})]});
f=w6.formOf(two(7,NO));ctx.ui.setForm(f);ctx.ui.drawSettings();
f=ctx.w6merge(two(null,NO),f,moved);ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([lineOff('p').checked,daily()],[true,[NO]],'a [chain] No cap is sent through a reorder of entries of one name');
f=ctx.w6merge(two(7),f,moved);ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([lineOff('p').checked,daily()],[false,[null]],'and an answer without it is followed');
// A CLI entry's own calls a day lifted by a [chain] no cap: a subscription's line has no field, and
// its switch shows on, so it can be turned off again; a save keeps it.
f=w6.formOf(settings('resident',{ranges,chain:[row('cli',{subscription:true,daily_budget:NO,effective_daily_budget:null,capped:false})]}));
ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([field('chain.cli.daily_budget'),lineOff('cli')?.checked,daily()],[undefined,true,[NO]],
  'a subscription lifted by [chain] shows its switch on; an unrelated save keeps its [chain] no cap');
// Loaded off: ticked, its field empty and set aside, said as saved; turned on again, a number is due.
f=w6.formOf(settings('resident',{ranges,paid_usd_per_month:null,
  my_work:{curate:false,calls_a_day:NO,estimate:{calls:[3,null],usd:[0.0123,null],priced:{name:'p',model:'m'}}}}));
ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual([field('paid_usd_per_month').value,field('paid_usd_per_month').disabled],['',true],'the month\'s limit set aside');
assert.deepEqual([field('my_work.calls_a_day').value,field('my_work.calls_a_day').disabled],['',true]);
shown=text(panel);
assert(!shown.includes(String(NO)),'4294967295 is never shown');
assert.equal(shown.split('Saved: No cap').length-1,1,'My Work\'s saved no cap is said');
assert(shown.includes('about 3, with no cap. At p (m) prices: about $0.012 a day; a busy day costs more.'),'the estimate with no most');
assert.deepEqual([w6.saveBody().body.paid_usd_per_month,w6.saveBody().body.my_work.calls_a_day],[null,NO],'caps left off stay off');
tick(off('my_work'),false);
assert.equal(w6.saveBody().field,'my_work.calls_a_day');
typeIn(field('my_work.calls_a_day'),'40');
assert.equal(w6.saveBody().body.my_work.calls_a_day,40);
ctx.ui.setLang('ja');ctx.ui.drawSettings();
shown=text(panel);
assert(shown.includes('上限なし')&&shown.includes('金額で止めるのは、各サービス側で決めた上限だけです。1 日の回数の上限は、そのまま効きます。'),'the Japanese switch and warning');
assert(shown.includes('およそ 3 回(上限なし)'),'the Japanese estimate with no most');
ctx.ui.setLang('en');
console.log('PASS: the caps\' No cap switches');
}
// Decision 69 (69b-2a): one settings page. The simple controls' saves and the general autosave go
// one at a time, each built from what is saved when its turn comes; what no body carries, and the
// confirmations waiting, outlive every answer; the focus goes back to the control it was on, on
// its side of the page; nothing outside the general forms starts a general save; a new home's
// resident choice counts as unsaved until a general save sends it (#803).
{
  vm.runInContext('globalThis.w7={bodyKey};',ctx);
  const {w7}=ctx;
  const w6fetch=ctx.fetch;
  let reread=null;
  ctx.fetch=(url,options)=>{
    if(options?.method==='POST'){posted.push({url,body:options.body});return Promise.resolve(answers.shift());}
    if(url.startsWith('/api/settings?')&&reread)return Promise.resolve(json(reread));
    return new Promise(()=>{});
  };
  const sent=()=>posted.map(p=>JSON.parse(p.body));
  // The event reaches the control's own form, as it bubbles there.
  const fire=(name,ev)=>{for(const cb of ev.target?.closest?.('form')?.listeners[name]||[])cb(ev);};
  const all=sel=>panel.querySelectorAll(sel);
  const controls=()=>[...all('input'),...all('select'),...all('textarea')];
  const simple=key=>[...controls(),...all('button')].find(n=>n.dataset.simpleFocus===key);
  const general=name=>controls().find(n=>n.dataset.field===name&&!n.dataset.simpleFocus);
  const action=name=>[...all('button'),...document.getElementById('status').querySelectorAll('button')].filter(n=>n.dataset.action===name).at(-1);
  const flip=(node,v)=>{node.checked=v;for(const cb of node.listeners.change||[])cb({target:node});};
  const leave=(node,next=null)=>{fire('change',{target:node});fire('focusout',{target:node,relatedTarget:next});if(next)next.focus();else document.activeElement=null;};
  const later=()=>{let resolve;const promise=new Promise(r=>{resolve=r;});return [promise,resolve];};
  const settled=async(n=4)=>{for(let i=0;i<n;i++)await settle();};
  const draw=s=>{vm.runInContext('lineDrafts.clear()',ctx);const g=w6.formOf(s);ctx.ui.setForm(g);ctx.ui.drawSettings();posted.length=0;return g;};
  // What the general save would send is what is saved: nothing waits unseen for the next one.
  const clean=()=>w7.bodyKey(w6.saveBody(true).body)===ctx.ui.getForm().savedKey;
  const status=()=>document.getElementById('status').textContent;
  const inGeneral=n=>Boolean(n.closest('.general-form'));
  const pageControls=document.querySelectorAll;document.querySelectorAll=sel=>panel.querySelectorAll(sel);
  const base=settings('resident');
  const curated={...base.summary,curate:true};
  const sized=n=>({...base.inject,session_start_chars:n});
  const row=(name,extra={})=>({name,on:true,key:'ok',key_file:null,effective_model:'model-'+name,model_rule:'any',model:null,model_applied:true,
    daily_budget:null,timeout_s:null,effective_timeout_s:90,differs:[],entries:1,subscription:false,budget_from_key:false,effective_daily_budget:200,...extra});
  const entry=(name,index)=>({name,selector:{source:'config',index},saved:{enabled:true,kind:'openai',base_url:'http://127.0.0.1:9/v1',
    model:'model-'+name,key:'none',timeout_s:60,limits:{}},effective:{on:true,order:index,model:'model-'+name,timeout_s:60}});
  const search={provider:'none',workers_ai:{account_id:null,key_file:'/home/made-up/CF_WORKERS_AI_KEY.md',key:'missing',daily_requests:200,
    monthly_usd:1,usd_this_month:0,requests_today:0},local:{unavailable:null,files:'not_downloaded',dir:'/home/made-up/.oboete/models/bge-m3',held:0}};
  const ai=(names,extra={})=>settings('resident',{chain:names.map(n=>row(n)),providers:names.map((n,i)=>entry(n,i)),
    embedding:search,ranges:{...base.ranges,daily_requests:[1,100000]},...extra});
  const masked=(rules,enabled=true)=>({enabled,extra_rules:rules,allowlist:[]});
  const rule={id:'acme-token',regex:'acme_[a-z0-9]{8}',keywords:[],entropy:null,secret_group:null};
  const judge={set:true,enabled:true,destination:'jev',destinations:[{id:'jev',key:'ok',usd_per_mtok_in:0.042,local:false}],key:'ok',
    key_file:'/home/made-up/.local/share/oboete/keys/k1',cloudflare_account_id:null,daily_budget:300,calls_today:0,usd_this_month:0};
  const phrase=()=>general('onboarding.phrase');

  // One page: no switch between two views; each group closes on its More settings, holding the
  // detailed controls that belong to it; This computer and Maintenance close the page.
  const privacy={available:true,repositories:[{label:'github.com/acme/live',selector:'r2',never_recorded:false},
    {label:'github.com/acme/old',selector:'r1',never_recorded:true}],folders:[],patterns:[],rescan:{state:'idle',processed:null,total:null}};
  // A key missing: the AI line shows its key field, as each entry in More settings does.
  let f=draw(ai(['alpha','beta'],{judge,privacy,key_input:true,chain:[row('alpha',{key:'missing'}),row('beta')]}));
  assert(!panel.querySelector('.settings-view-switch'),'no switch between two views');
  assert.equal(vm.runInContext('typeof settingsMode',ctx),'undefined','no view is kept');
  const more=key=>panel.querySelector(`.group-s-${key}`)?.querySelector('.more-settings');
  const fieldsIn=node=>[...node.querySelectorAll('input'),...node.querySelectorAll('select'),...node.querySelectorAll('textarea')].map(n=>n.dataset.field).filter(Boolean);
  const holds=(key,fields,classes=[])=>{
    assert(more(key),`${key} has its More settings`);
    for(const name of fields)assert(fieldsIn(more(key)).includes(name),`${key}'s More settings holds ${name}`);
    for(const cls of classes)assert(more(key).querySelector(cls),`${key}'s More settings holds ${cls}`);
  };
  holds('inject',['inject.session_start_chars','inject.session_start_note','inject.session_start_list','inject.per_prompt_chars',
    'inject.per_prompt_wait_ms','inject.correction_chars']);
  holds('ai',['summary.language','summary.window_tokens','summary.idle_minutes','gemini'],['.provider-entries','.stopped-state']);
  // Each AI's model, calls a day and timeout are its line's (69b-3c), outside More settings.
  const alphaLine=panel.querySelectorAll('.simple-provider').find(n=>n.dataset.providerName==='alpha');
  for(const name of ['chain.alpha.model','chain.alpha.daily_budget','chain.alpha.timeout_s']){
    assert(fieldsIn(alphaLine).includes(name)&&!fieldsIn(more('ai')).includes(name),`the AI line holds ${name}, More settings does not`);
  }
  holds('my_work',['my_work.calls_a_day','my_work.no_cap'],['.judge-settings']);
  holds('search',['embedding.account_id','embedding.daily_requests','embedding.monthly_usd'],['.workers-ai']);
  holds('privacy',['retention.forever','redaction.allowlist'],['.capture-settings','.never-more','.retention']);
  assert(text(more('privacy').querySelector('.never-more')).includes('github.com/acme/live')
    &&!text(more('privacy').querySelector('.never-more')).includes('github.com/acme/old'),'More settings lists the repositories recorded');
  assert(text(panel.querySelector('.never-record')).includes('github.com/acme/old')
    &&!text(panel.querySelector('.never-record')).includes('github.com/acme/live'),'the group lists those never recorded');
  holds('agents',['preference.text'],['.preference','.agent-setup','.agent-readiness']);
  for(const name of ['worker.resident','view.port','backup.dir'])assert(fieldsIn(panel.querySelector('.group-computer')).includes(name),`This computer holds ${name}`);
  for(const cls of ['.view-address','.hidden-settings','.doctor-diagnostics','.maintenance'])assert(panel.querySelector('.group-maintenance').querySelector(cls),`Maintenance holds ${cls}`);
  assert(!more('ai').querySelectorAll('button').some(b=>text(b)==='Add provider'),'one way to add an AI: the catalog');
  assert.equal(more('ai').querySelectorAll('th').length,0,'no chain table: its fields are the lines\'');
  // What the simple controls own is theirs alone (decision 3); every control is drawn once, told
  // by its simple focus, or by its field and its AI entry, inside the general forms or not (4d).
  const generalFields=new Set(controls().filter(inGeneral).map(n=>n.dataset.field).filter(Boolean));
  for(const owned of ['summary.curate','my_work.curate','paid_usd_per_month','paid_usd_per_month.no_cap','inject.session_start',
    'inject.per_prompt','redaction.enabled','chain.alpha.on','embedding.choice','never.folder'])
    assert(!generalFields.has(owned),`${owned} is the simple control's alone`);
  const seen=new Map();
  for(const n of controls().filter(n=>n.dataset.field||n.dataset.simpleFocus)){
    const key=n.dataset.simpleFocus?`simple ${n.dataset.simpleFocus}`
      :[n.dataset.field,n.closest('.provider-entry')?.dataset.selector??''].join(' ');
    seen.set(key,(seen.get(key)||0)+1);
  }
  assert.deepEqual([...seen].filter(([,n])=>n>1),[],'each control once');
  assert(panel.querySelectorAll('.simple-block').every(n=>!n.closest('form')),'no simple control is inside a form');
  ctx.ui.setLang('ja');ctx.ui.drawSettings();
  assert(text(panel).includes('細かい設定')&&text(panel).includes('このパソコン')&&text(panel).includes('診断と保守'),'Japanese');
  for(const key of ['more_settings','never_recorded_none','never_repos_h','simple_never_more','simple_entry_detail','inject_chars','per_prompt_chars']){
    const [en,ja]=[ctx.tier.text(key,'en'),ctx.tier.text(key,'ja')];
    assert(en&&ja&&en!==ja,`${key} in both languages`);
  }
  ctx.ui.setLang('en');ctx.ui.drawSettings();
  assert(text(panel).includes('More settings')&&text(panel).includes('This computer'),'English');

  // A link to a detailed control opens the folds around it, its group's More settings among them,
  // and focuses it.
  const opened=node=>{
    if(!node)return false;
    for(let d=node.parentElement?.closest('details');d;d=d.parentElement?.closest('details'))if(!d.open)return false;
    return true;
  };
  f=draw(ai(['alpha']));
  panel.querySelectorAll('.simple-provider').find(n=>n.dataset.providerName==='alpha').querySelectorAll('button')
    .find(b=>b.className.includes('simple-link')).click();
  assert(document.activeElement?.className.includes('provider-entry')&&document.activeElement.closest('.more-settings'),
    'the AI line\'s link focuses its entry in More settings');
  assert(opened(document.activeElement),'with the folds around it open');
  f=draw(ai(['alpha']));
  const customAt=vm.runInContext('AI_CATALOG.findIndex(entry=>!entry.name)',ctx);
  panel.querySelector('.simple-catalog').querySelectorAll('button')[customAt].click();
  assert(document.activeElement?.className.includes('new-provider')&&opened(document.activeElement),
    'the catalog\'s own server opens the new entry\'s form in More settings');
  document.activeElement=null;

  // A simple switch clicked while a general save is on its way waits for its answer, disabled
  // with the page inert, then sends with that answer's version and what it saved.
  f=draw(ai(['alpha']));
  let [answer,give]=later();answers=[answer];
  typeIn(general('my_work.calls_a_day'),'200');leave(general('my_work.calls_a_day'));await settle();
  assert.equal(posted.length,1,'the calls a day are sent');
  const myWork=simple('my_work.curate');flip(myWork,true);await settle();
  assert.equal(posted.length,1,'a simple switch clicked meanwhile waits');
  assert.deepEqual([myWork.disabled,panel.querySelector('.settings').inert],[true,true],'disabled, with the page');
  let [simpleAnswer,giveSimple]=later();answers=[simpleAnswer];
  give(json(ai(['alpha'],{version:'v2',my_work:{curate:false,calls_a_day:200,estimate:null}})));await settled();
  assert.equal(posted.length,2,'then it is sent');
  assert.equal(sent()[1].version,'v2','with the version the save gave');
  assert.deepEqual(sent()[1].my_work,{curate:true,calls_a_day:200},'and the calls a day it saved');
  assert.deepEqual([simple('my_work.curate').disabled,panel.querySelector('.settings').inert],[true,true],
    'the page the answer drew again waits for it, its switch too');
  giveSimple(json(ai(['alpha'],{version:'v3',my_work:{curate:true,calls_a_day:200,estimate:null}})));await settled();
  assert.deepEqual([simple('my_work.curate').disabled,Boolean(panel.querySelector('.settings').inert)],[false,false],'and is usable after it');
  assert(clean(),'nothing is left to save');
  // ...and masking off confirmed meanwhile keeps the rule the save changed.
  f=draw(ai(['alpha'],{redaction:masked([rule])}));
  [answer,give]=later();answers=[answer];
  const regex=()=>general('redaction.extra_rules.0.regex');
  typeIn(regex(),'acme_[a-z0-9]{12}');leave(regex());await settle();
  assert.equal(posted.length,1);
  flip(simple('redaction.enabled'),false);simple('mask.confirm').click();await settle();
  assert.equal(posted.length,1,'masking off confirmed meanwhile waits');
  const changed={...rule,regex:'acme_[a-z0-9]{12}'};
  answers=[json(ai(['alpha'],{version:'v3',redaction:masked([changed],false)}))];
  give(json(ai(['alpha'],{version:'v2',redaction:masked([changed])})));await settled();
  assert.equal(posted.length,2);
  assert.deepEqual([sent()[1].version,sent()[1].redaction.enabled,sent()[1].redaction.extra_rules[0].regex],['v2',false,'acme_[a-z0-9]{12}'],
    'masking off keeps the rule the save changed');
  // A general change made during a simple save waits for it, and is sent with its answer's version.
  f=draw(ai(['alpha']));
  [answer,give]=later();answers=[answer];
  flip(simple('summary.curate'),true);await settle();
  assert.equal(posted.length,1,'the simple switch is sent');
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settle();
  assert.equal(posted.length,1,'a general change made meanwhile waits for it');
  answers=[json(ai(['alpha'],{version:'v3',summary:curated,inject:sized(4000)}))];
  give(json(ai(['alpha'],{version:'v2',summary:curated})));await settled();
  assert.equal(posted.length,2,'then it is sent');
  assert.deepEqual([sent()[1].version,sent()[1].inject.session_start_chars,sent()[1].summary.curate],['v2',4000,true],
    'with the version and what the simple save saved');
  // A general save refused as stale reads the settings again; a simple save waiting sends what was read.
  f=draw(ai(['alpha']));
  [answer,give]=later();answers=[answer];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settle();
  flip(simple('summary.curate'),true);await settle();
  assert.equal(posted.length,1);
  reread=ai(['alpha'],{version:'v9',inject:sized(5000)});
  answers=[json(ai(['alpha'],{version:'v10',summary:curated,inject:sized(5000)}))];
  give(json({code:'stale'},409));await settled(6);
  assert.equal(posted.length,2,'after the stale save is read again, the simple save is sent');
  assert.deepEqual([sent()[1].version,sent()[1].inject.session_start_chars,sent()[1].summary.curate],['v9',5000,true],'with what was read');
  reread=null;

  // What no body carries outlives a simple save: the guide's phrase is in no body and stays.
  f=draw(ai(['alpha']));
  typeIn(phrase(),'a made-up phrase');fire('input',{target:phrase()});leave(phrase());await settle();
  assert.equal(posted.length,0,'the guide\'s phrase starts no save');
  answers=[json(ai(['alpha'],{version:'v2',summary:curated}))];
  flip(simple('summary.curate'),true);await settled();
  assert.equal(posted.length,1);
  assert(!posted[0].body.includes('a made-up phrase'),'the phrase is in no body');
  assert.equal(phrase().value,'a made-up phrase','and stays through the answer');
  // The masking-off and no-cap confirmations open stay open through a general save and a provider
  // write, and still work.
  f=draw(ai(['alpha'],{paid_usd_per_month:5,redaction:masked([])}));
  flip(simple('redaction.enabled'),false);flip(simple('paid_usd_per_month.no_cap'),true);
  assert(simple('mask.confirm')&&simple('cap.confirm'),'both confirmations open');
  answers=[json(ai(['alpha'],{version:'v2',paid_usd_per_month:5,redaction:masked([]),inject:sized(4000)}))];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
  assert.equal(posted.length,1);
  assert(simple('mask.confirm')&&simple('cap.confirm'),'they stay open through a general save');
  answers=[json(ai(['alpha'],{version:'v3',paid_usd_per_month:5,redaction:masked([]),inject:sized(4000)}))];
  await w6.providerOperation({op:'enabled',selector:{source:'config',index:0},enabled:true},action('port.move'));await settled();
  assert.deepEqual(posted.map(p=>p.url),['/api/settings','/api/providers']);
  assert(simple('mask.confirm')&&simple('cap.confirm'),'and through a provider write');
  answers=[json(ai(['alpha'],{version:'v4',paid_usd_per_month:5,redaction:masked([],false),inject:sized(4000)}))];
  simple('mask.confirm').click();await settled();
  assert.deepEqual([sent()[2].version,sent()[2].redaction.enabled],['v3',false],'masking off still works');
  answers=[json(ai(['alpha'],{version:'v5',paid_usd_per_month:null,redaction:masked([],false),inject:sized(4000)}))];
  simple('cap.confirm').click();await settled();
  assert.deepEqual([sent()[3].version,sent()[3].paid_usd_per_month],['v4',null],'and so does no cap');
  // ...and a finite cap opened while no cap is saved.
  f=draw(ai(['alpha'],{paid_usd_per_month:null}));
  flip(simple('paid_usd_per_month.no_cap'),false);
  assert.equal(simple('paid_usd_per_month').disabled,false,'a cap can be typed');
  answers=[json(ai(['alpha'],{version:'v2',paid_usd_per_month:null,inject:sized(4000)}))];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
  assert.equal(simple('paid_usd_per_month').disabled,false,'it stays open through a general save');
  answers=[json(ai(['alpha'],{version:'v3',paid_usd_per_month:3,inject:sized(4000)}))];
  const limit=simple('paid_usd_per_month');typeIn(limit,'3');for(const cb of limit.listeners.blur)cb();await settled();
  assert.deepEqual([sent()[1].version,sent()[1].paid_usd_per_month],['v2',3],'and saves the cap typed');
  // A saved rule removed waits in the status line; masking off confirmed inline meanwhile keeps the
  // rule, and the removal is asked again, and still works both ways.
  for(const way of ['confirm','cancel']){
    f=draw(ai(['alpha'],{redaction:masked([rule])}));
    const remove=all('button').find(n=>text(n)==='Remove this rule');
    remove.click();fire('click',{target:remove});await settle();
    assert.equal(posted.length,0);assert(status().includes('acme-token'),'a saved rule removed waits');
    flip(simple('redaction.enabled'),false);
    answers=[json(ai(['alpha'],{version:'v2',redaction:masked([rule],false)}))];
    simple('mask.confirm').click();await settled();
    assert.equal(posted.length,1);
    assert.deepEqual([sent()[0].redaction.enabled,sent()[0].redaction.extra_rules.map(r=>r.id)],[false,['acme-token']],
      'masking off is sent with the saved rule');
    assert(status().includes('acme-token'),'the removal is asked again');
    if(way==='confirm'){
      answers=[json(ai(['alpha'],{version:'v3',redaction:masked([],false)}))];
      action('held.confirm').click();await settled();
      assert.deepEqual([sent()[1].version,sent()[1].redaction.extra_rules,sent()[1].redaction.enabled],['v2',[],false],
        'confirmed, the rule is removed, masking as saved');
    }else{
      action('held.cancel').click();await settle();
      assert.equal(posted.length,1,'cancelled, nothing is sent');
      assert.equal(general('redaction.extra_rules.0.id').value,'acme-token','and the rule is back');
    }
  }

  // The focus goes back where it was through a redraw: the AI line's other model, typed or emptied,
  // with its caret (69b-3c); the AI line's model choice to its own.
  f=draw(ai(['alpha']));
  const pickModel=v=>{const s=simple('model:alpha');s.value=v;for(const cb of s.listeners.change||[])cb({target:s});};
  pickModel('__other');
  const typedModel=()=>simple('model-text:alpha');
  assert.equal(document.activeElement,typedModel(),'another model opens its field, focused');
  for(const typed of ['other-model','']){
    const [start,end]=typed?[3,5]:[0,0];
    typedModel().focus();typeIn(typedModel(),typed);typedModel().setSelectionRange(start,end);
    ctx.ui.drawSettings();
    assert.equal(document.activeElement,typedModel(),`the AI line's other model (${typed||'emptied'}) has the focus back`);
    assert.deepEqual([typedModel().value,typedModel().selectionStart,typedModel().selectionEnd],[typed,start,end],'with its text and its caret');
  }
  vm.runInContext('lineDrafts.clear()',ctx);ctx.ui.drawSettings();
  simple('model:alpha').focus();ctx.ui.drawSettings();
  assert.equal(document.activeElement,simple('model:alpha'),'the AI line\'s model choice keeps its own');
  // A field each AI entry repeats goes back to its own entry's, not to another's or the AI line's.
  f=draw(ai(['alpha','beta'],{key_input:true,chain:['alpha','beta'].map(n=>row(n,{key:'missing'}))}));
  assert(simple('key:beta'),'the AI line has its key field too');
  const keyOf=name=>more('ai').querySelectorAll('.provider-entry').find(n=>text(n).includes(name))
    ?.querySelectorAll('input').find(n=>n.dataset.field==='providers.key');
  assert(keyOf('beta')&&keyOf('alpha')!==keyOf('beta'),'each entry has its key field');
  keyOf('beta').focus();ctx.ui.drawSettings();
  assert.equal(document.activeElement,keyOf('beta'),'the second entry\'s key field has the focus back');
  const modelOf=name=>more('ai').querySelectorAll('.provider-entry').find(n=>text(n).includes(name))
    ?.querySelectorAll('input').find(n=>n.dataset.field==='providers.model');
  // A key refused is marked on the field it was typed in: the AI line's, not its entry's in More
  // settings (Codex's review of 69b-2a).
  answers=[json({code:'bad_key',field:'providers.key'},422)];
  const lineKey=()=>simple('key:alpha');
  typeIn(lineKey(),'a made-up wrong key');simple('key-save:alpha').click();await settled();
  assert.equal(posted.at(-1).url,'/api/providers/key');
  assert.deepEqual([lineKey().attrs['aria-invalid'],keyOf('alpha').attrs['aria-invalid'],document.activeElement===lineKey()],['true',undefined,true],
    'the AI line\'s key field is marked and focused');
  answers=[json({code:'bad_key',field:'providers.key'},422)];
  typeIn(keyOf('alpha'),'another made-up key');more('ai').querySelectorAll('button').find(b=>b.dataset.simpleFocus?.startsWith('key-save:entry:')&&b.closest('.provider-entry')===keyOf('alpha').closest('.provider-entry')).click();await settled();
  assert.deepEqual([keyOf('alpha').attrs['aria-invalid'],document.activeElement===keyOf('alpha')],['true',true],'and an entry\'s on the entry');
  posted.length=0;document.activeElement=null;
  modelOf('beta').focus();modelOf('beta').setSelectionRange(2,4);ctx.ui.drawSettings();
  assert.deepEqual([document.activeElement===modelOf('beta'),modelOf('beta').selectionStart],[true,2],
    'and so does its editor\'s model, which each entry repeats');
  // The guide's phrase and the preferences' text, typed while a general save is on its way, keep
  // their text, focus and caret through its answer, and start no save.
  f=draw(ai(['alpha']));
  [answer,give]=later();answers=[answer];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settle();
  phrase().focus();typeIn(phrase(),'made-up words');phrase().setSelectionRange(4,4);fire('input',{target:phrase()});
  give(json(ai(['alpha'],{version:'v2',inject:sized(4000)})));await settled();
  assert.deepEqual([document.activeElement===phrase(),phrase().value,phrase().selectionStart],[true,'made-up words',4],
    'the guide\'s phrase keeps its text, focus and caret');
  const preference=()=>general('preference.text');
  [answer,give]=later();answers=[answer];
  typeIn(general('inject.session_start_chars'),'4500');leave(general('inject.session_start_chars'));await settle();
  preference().focus();typeIn(preference(),'made-up preference');preference().setSelectionRange(6,6);fire('input',{target:preference()});
  give(json(ai(['alpha'],{version:'v3',inject:sized(4500)})));await settled();
  assert.deepEqual([document.activeElement===preference(),preference().value,preference().selectionStart],[true,'made-up preference',6],
    'the preferences\' text keeps its text, focus and caret');
  assert.equal(posted.length,2,'and neither starts a save');
  document.activeElement=null;

  // An arrow waiting for a save on its way moves its AI past the next one in use in the order saved
  // when its turn comes; an AI gone meanwhile moves nothing.
  f=draw(ai(['alpha','beta','gamma']));
  [answer,give]=later();answers=[answer];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settle();
  simple('move:beta:1').click();await settle();
  assert.equal(posted.length,1);
  answers=[json(ai(['alpha','beta','gamma'],{version:'v3',inject:sized(4000)}))];
  give(json(ai(['beta','alpha','gamma'],{version:'v2',inject:sized(4000)})));await settled();
  assert.equal(posted.length,2);
  assert.deepEqual(sent()[1].chain.map(r=>r.name),['alpha','beta','gamma'],'beta moves past alpha, and nothing else moves');
  f=draw(ai(['alpha','beta','gamma']));
  [answer,give]=later();answers=[answer];
  typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settle();
  simple('move:beta:1').click();await settle();
  give(json(ai(['alpha','gamma'],{version:'v2',inject:sized(4000)})));await settled();
  assert.equal(posted.length,1,'an AI gone meanwhile moves nothing');
  // An AI whose entries are all off is not in use, though its name's switch is on: an arrow moves
  // past it, as the lines show it (Codex's review of 69b-2a).
  const offEntry={...entry('beta',1),saved:{...entry('beta',1).saved,enabled:false}};
  const offBeta=(order,extra={})=>ai(order,{providers:[entry('alpha',0),offEntry,entry('gamma',2)],...extra});
  f=draw(offBeta(['alpha','beta','gamma']));
  assert.deepEqual(panel.querySelector('.simple-providers').querySelectorAll('.simple-provider').map(n=>n.dataset.providerName),['alpha','gamma'],
    'beta, its entry off, is not in use');
  answers=[json(offBeta(['gamma','beta','alpha'],{version:'v2'}))];
  simple('move:gamma:-1').click();await settled();
  assert.deepEqual(sent()[0].chain.map(r=>r.name),['gamma','beta','alpha'],'gamma moves past alpha, the AI in use above it');

  // Nothing outside the general forms starts a general save: the preferences have their own form,
  // in no other; the maintenance actions and the agents' setup are in none.
  f=draw(ai(['alpha']));
  const preferences=preference().closest('form');
  assert(preferences&&!preferences.parentElement.closest('form'),'the preferences\' form is in no other form');
  for(const b of [...panel.querySelector('.group-maintenance').querySelectorAll('button'),...panel.querySelector('.agent-setup').querySelectorAll('button')])
    assert(!b.closest('form'),'maintenance and agent setup actions are in no form');
  for(const cb of preferences.listeners.submit||[])await cb({target:preferences,preventDefault(){}});
  await settle();
  assert.equal(posted.length,0,'the preferences\' submit sends no general save');
  // The AI line's "the entry's own" clears a name's model in `[chain]`, as the chain table's emptied
  // column did (69b-3c).
  f=draw(ai(['alpha'],{chain:[row('alpha',{model:'pinned-model',effective_model:'pinned-model'})]}));
  answers=[json(ai(['alpha'],{version:'v2'}))];
  pickModel('__own');await settled();
  assert.equal(sent()[0].chain[0].model,null,'the entry\'s own clears the name\'s model');
  // A model typed on the line and refused is marked on the field it was typed in, not on the line's
  // choice (Codex's review of 69b-2a).
  f=draw(ai(['alpha']));
  answers=[json({code:'bad_request',field:'chain.alpha.model'},400)];
  pickModel('__other');typeIn(typedModel(),'bad model');for(const cb of typedModel().listeners.blur)cb();await settled();
  assert.deepEqual([typedModel().attrs['aria-invalid'],simple('model:alpha').attrs['aria-invalid'],document.activeElement===typedModel()],['true',undefined,true],
    'the refused model is marked and focused where it was typed');
  vm.runInContext('lineDrafts.clear()',ctx);document.activeElement=null;

  // #803: a new home shows the resident worker on, as recommended, unsaved. A simple save leaves
  // it so; the guide's tier, phrase, jumps and search send nothing; its Apply saves the resident
  // choice shown with what it applies, even when that changes nothing else.
  const free={...entry('alpha',0),selector:{source:'builtin',index:0},saved:{enabled:true,kind:'openai',endpoint_supported:true,
    base_url:'https://free.example.invalid/v1',model:'model-alpha',key:'ok',timeout_s:60,limits:{usd_per_mtok_in:0,usd_per_mtok_out:0}}};
  const home=(extra={})=>ai(['alpha'],{first_run:true,worker:{resident:false},providers:[free],...extra});
  f=draw(home());
  assert.equal(general('worker.resident').checked,true,'a new home shows the resident worker on');
  assert(!clean(),'the key of what is saved takes it as saved, off');
  assert.equal(panel.querySelector('.group-computer').open,true,'This computer is open');
  answers=[json(home({version:'v2',first_run:false,summary:curated}))];
  flip(simple('summary.curate'),true);await settled();
  assert.equal(sent()[0].worker,undefined,'a simple save sends no resident choice');
  assert.equal(general('worker.resident').checked,true,'the choice shown stays');
  assert(!clean(),'unsaved');
  assert.equal(panel.querySelector('.group-computer').open,true,'This computer stays open while it is');
  const tier=controls().find(n=>n.dataset.field==='onboarding.tier');
  tier.value='free';for(const cb of tier.listeners.change||[])cb({target:tier});fire('change',{target:tier});
  typeIn(phrase(),'made-up');fire('input',{target:phrase()});leave(phrase());await settle();
  for(const b of panel.querySelector('.onboarding').querySelectorAll('button')){if(!b.dataset.action)b.click();fire('click',{target:b});}
  await settle();
  assert.equal(posted.length,1,'the guide\'s tier, phrase, jumps and search send nothing');
  answers=[json(home({version:'v3',first_run:false,summary:curated,worker:{resident:true}}))];
  action('onboarding.copy').click();await settled();
  assert.equal(posted.length,2,'its Apply saves');
  assert.deepEqual([sent()[1].version,sent()[1].worker.resident,sent()[1].summary.curate],['v2',true,true],
    'with the resident choice shown, though nothing else changed');
  assert(clean(),'after it, nothing waits');
  assert.equal(general('worker.resident').checked,true,'and the switch shows what is saved');

  // Decision 3: after each kind of simple save, the general save has nothing to send.
  {
    let saved=ai(['groq','beta'],{version:'s1',paid_usd_per_month:5,redaction:masked([])});
    ctx.fetch=(url,options)=>{
      if(options?.method!=='POST')return new Promise(()=>{});
      posted.push({url,body:options.body});
      const body=JSON.parse(options.body);
      const prior=name=>saved.chain.find(e=>e.name===name);
      const chain=body.chain?body.chain.map(r=>({...prior(r.name),...r,effective_model:r.model||prior(r.name).effective_model})):saved.chain;
      saved={...saved,...body,chain,version:'s'+(posted.length+1)};
      return Promise.resolve(json(saved));
    };
    f=draw(saved);
    const steps=[
      ['summaries on',()=>flip(simple('summary.curate'),true)],
      ['My Work on',()=>flip(simple('my_work.curate'),true)],
      ['session start off',()=>flip(simple('inject.session_start'),false)],
      ['each prompt on',()=>flip(simple('inject.per_prompt'),true)],
      ['an AI off',()=>flip(simple('on:beta'),false)],
      ['an AI on',()=>flip(simple('on:beta'),true)],
      ['an AI moved',()=>simple('move:beta:-1').click()],
      ['a model chosen',()=>{const m=simple('model:groq');m.value='openai/gpt-oss-20b';for(const cb of m.listeners.change)cb({target:m});}],
      ['the monthly cap typed',()=>{const c=simple('paid_usd_per_month');typeIn(c,'7');for(const cb of c.listeners.blur)cb();}],
      ['masking off',()=>{flip(simple('redaction.enabled'),false);simple('mask.confirm').click();}],
      ['masking on',()=>flip(simple('redaction.enabled'),true)],
      ['no cap',()=>{flip(simple('paid_usd_per_month.no_cap'),true);simple('cap.confirm').click();}],
    ];
    for(const [label,step] of steps){
      const before=posted.length;
      step();await settled();
      assert.equal(posted.length,before+1,`${label} is saved`);
      assert(clean(),`after ${label}, the general save has nothing to send`);
    }
  }
  // 69b-2b: Settings is drawn before the privacy state is read, which on a large store takes most
  // of a second; until it is, and while it is read again after a save, the privacy blocks say it is
  // being read, not that it cannot be.
  {
    const [reading,unreadable]=['privacy_reading','privacy_unavailable'].map(key=>ctx.tier.text(key,'en'));
    ctx.ui.setLang('en');
    let [privacyAnswer,givePrivacy]=later();
    let settingsNow=ai(['alpha']);
    // The saves are answered from `answers`, as in the rest of this block.
    const mock=w6fetch;
    ctx.fetch=(url,options)=>{
      if(!options?.method&&url.startsWith('/api/settings?'))return Promise.resolve(json(settingsNow));
      if(!options?.method&&url.startsWith('/api/hidden'))return Promise.resolve(json({items:[]}));
      if(!options?.method&&url.startsWith('/api/privacy?'))return privacyAnswer;
      return mock(url,options);
    };
    ctx.ui.setView('settings');
    const render=await ctx.ui.showSettings();render();posted.length=0;
    const group=()=>panel.querySelector('.group-s-privacy');
    assert(text(group()).includes(reading)&&!text(group()).includes(unreadable),'drawn before the privacy read, which is said as being read');
    assert.equal(ctx.ui.getForm().savedSettings.privacy,undefined,'what is saved, the viewer\'s answer, carries no privacy state');
    assert(more('privacy').querySelector('.never-more').hidden,'its More settings part waits hidden');
    givePrivacy(json(privacy));await settled();
    assert(!text(group()).includes(reading)&&text(more('privacy').querySelector('.never-more')).includes('github.com/acme/live')
      &&!more('privacy').querySelector('.never-more').hidden,'then drawn with it');
    assert(text(group().querySelector('.group-desc')).includes('1 place'),'its heading counts the places never recorded');
    [privacyAnswer,givePrivacy]=later();
    answers=[json(ai(['alpha'],{version:'v2',inject:sized(4000)}))];
    typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
    assert.equal(posted.length,1);
    assert(text(group()).includes(reading)&&!text(group()).includes(unreadable),'read again after a save, said as being read meanwhile');
    givePrivacy(json({},503));await settled();
    assert(text(group()).includes(unreadable)&&!text(group()).includes(reading),'a read that fails says so');
    const counted=()=>!text(group()).includes(reading)&&!text(group()).includes(unreadable)&&text(group().querySelector('.group-desc')).includes('1 place');
    // A provider's write answered while the first read is on its way: the form it draws takes that
    // read when it comes (Codex and cubic on #806).
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    answers=[json(ai(['alpha'],{version:'v2'}))];
    await w6.providerOperation({op:'enabled',selector:{source:'config',index:0},enabled:true},action('port.move'));await settled();
    assert.deepEqual([posted.map(p=>p.url),text(group()).includes(reading)],[['/api/providers'],true],'still being read after the write');
    givePrivacy(json(privacy));await settled();
    assert(counted(),'the form the write drew takes the read');
    // A confirmation cancelled keeps the privacy state read; one cancelled while the first read is
    // on its way takes the read when it comes (cubic and CodeRabbit on #806).
    settingsNow=ai(['alpha'],{redaction:masked([rule])});
    const removeAndCancel=async()=>{
      const remove=all('button').find(n=>text(n)==='Remove this rule');
      remove.click();fire('click',{target:remove});await settle();
      assert(status().includes('acme-token'),'a saved rule removed waits');
      action('held.cancel').click();await settle();
    };
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    givePrivacy(json(privacy));await settled();
    await removeAndCancel();
    assert(counted(),'a confirmation cancelled keeps the privacy state read');
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    await removeAndCancel();
    assert(text(group()).includes(reading),'cancelled while it is read, it is still being read');
    givePrivacy(json(privacy));await settled();
    assert(counted(),'and the form the cancel drew takes the read');
    assert.equal(posted.length,0,'nothing was sent');
    // Two reads on their way (the first, and Refresh pressed meanwhile), then a confirmation
    // cancelled: only the later read is drawn, whichever comes first (Codex and CodeRabbit on #806).
    const refreshButton=()=>all('button').find(n=>text(n)==='Refresh privacy state');
    let giveFirst;[privacyAnswer,giveFirst]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();
    await removeAndCancel();
    giveFirst(json({},503));await settled();
    assert(text(group()).includes(reading)&&!text(group()).includes(unreadable),'the earlier read, come first, is not drawn');
    givePrivacy(json(privacy));await settled();
    assert(counted(),'the later one is');
    // Refresh pressed while the first read is on its way, then a provider's write and a saved rule
    // removed: the read the write's form takes leaves that form's confirmation in the status line
    // (cubic on #806).
    [privacyAnswer,giveFirst]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();
    answers=[json(ai(['alpha'],{version:'v2',redaction:masked([rule])}))];
    await w6.providerOperation({op:'enabled',selector:{source:'config',index:0},enabled:true},action('port.move'));await settled();
    const remove=all('button').find(n=>text(n)==='Remove this rule');
    remove.click();fire('click',{target:remove});await settle();
    assert(status().includes('acme-token'),'a saved rule removed waits');
    givePrivacy(json(privacy));await settled();
    assert(counted()&&status().includes('acme-token'),'the read the write\'s form took leaves its confirmation');
    giveFirst(json(privacy));await settled();
    action('held.cancel').click();await settle();
    // #808: a write that holds the page, started while the read is on its way, keeps its hold when
    // the read comes first: the state read is drawn in place, and a field the write refuses is
    // marked on the page shown (Codex on #806).
    settingsNow=ai(['alpha']);
    const held=()=>panel.querySelector('.settings').inert===true;
    const timeoutField=()=>panel.querySelectorAll('.provider-entry').find(n=>n.dataset.selector==='config:0')
      .querySelectorAll('input').find(n=>n.dataset.field==='providers.timeout_s');
    for(const how of ['ok','refused','failed']){
      [privacyAnswer,givePrivacy]=later();
      (await ctx.ui.showSettings())();posted.length=0;
      const [written,giveWrite]=later();
      answers=[written];
      const write=w6.providerOperation({op:'move',selector:{source:'config',index:0},to:0},action('port.move'));
      await settled();
      assert(held()&&text(group()).includes(reading),`${how}: the write holds the page while the read is on its way`);
      givePrivacy(json(privacy));await settled();
      assert(held()&&counted(),`${how}: the read come first is drawn, the page still held`);
      if(how==='ok')giveWrite(json(ai(['alpha'],{version:'v2'})));
      else if(how==='refused')giveWrite(json({field:'providers.timeout_s'},400));
      else giveWrite(Promise.reject(new TypeError('made-up network failure')));
      await write;await settled();
      assert(!held()&&counted(),`${how}: once the write has ended the page is usable and shows the state read`);
      if(how==='refused')assert.equal(timeoutField().attrs['aria-invalid'],'true','the refused field is marked on the page shown');
    }
    // A simple change saved while the read is on its way holds the page through the read too.
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    let [written,giveWrite]=later();
    answers=[written];
    flip(simple('inject.session_start'),false);await settled();
    assert.equal(posted.length,1);
    givePrivacy(json(privacy));await settled();
    assert(held()&&counted(),'a simple change keeps its hold when the read comes first');
    [privacyAnswer,givePrivacy]=later();
    giveWrite(json(ai(['alpha'],{version:'v2',inject:{...ai(['alpha']).inject,session_start:false}})));await settled();
    givePrivacy(json(privacy));await settled();
    assert(!held()&&counted(),'its answer draws the page, which reads the state again');
    // A confirmation asked while Refresh reads stays, with its buttons, when the read ends; a
    // Refresh with nothing said meanwhile clears the status line (CodeRabbit on #806).
    settingsNow=ai(['alpha'],{redaction:masked([rule])});
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    givePrivacy(json(privacy));await settled();
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();
    const removeRule=all('button').find(n=>text(n)==='Remove this rule');
    removeRule.click();fire('click',{target:removeRule});await settle();
    assert(status().includes('acme-token'),'a saved rule removed waits');
    givePrivacy(json(privacy));await settled();
    assert(status().includes('acme-token')&&action('held.cancel'),'the read\'s end leaves the confirmation and its buttons');
    action('held.cancel').click();await settle();
    vm.runInContext("setStatus('A made-up note.')",ctx);
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();givePrivacy(json(privacy));await settled();
    assert.equal(status(),'','a Refresh with nothing said meanwhile clears the status line');
    // A field of a part drawn again keeps the focus and what is typed in it.
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();
    simple('never.folder').focus();typeIn(simple('never.folder'),'made-up-folder');
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([document.activeElement===simple('never.folder'),simple('never.folder').value],[true,'made-up-folder'],
      'the folder being typed keeps the focus and its text');
    document.activeElement=null;
    assert.equal(posted.length,0,'nothing was sent');
    // A place never recorded, added: its block, drawn in place, says it is saved, and no other
    // block still does, as when the page was drawn again.
    [privacyAnswer,givePrivacy]=later();
    answers=[json(ai(['alpha'],{version:'v2',inject:{...ai(['alpha']).inject,session_start:false}}))];
    flip(simple('inject.session_start'),false);await settled();
    givePrivacy(json(privacy));await settled();
    const receiptOf=name=>text(panel.querySelectorAll('.simple-block').find(n=>n.dataset.simpleBlock===name).querySelector('.simple-saved')).trim();
    assert(receiptOf('inject'),'a simple change says it is saved');
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:true})];
    typeIn(simple('never.folder'),'made-up-folder');
    all('button').find(n=>text(n)===ctx.tier.text('never_folder_add','en')).click();await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.at(-1).url,Boolean(receiptOf('never')),receiptOf('inject')],['/api/privacy/capture',true,''],
      'the place added says it is saved, and no other block still does');
    // #807: a secret being typed stays in its field, its button enabled, through the privacy read's
    // answer (drawn in place) and the page drawn again by a save's; it is in no form, storage,
    // request or attribute but its own save's.
    const keyed=(extra={})=>ai(['alpha','beta'],{key_input:true,chain:[row('alpha',{key:'missing'}),row('beta')],...extra});
    settingsNow=keyed();
    const stored=[];const setItem=ctx.localStorage.setItem;ctx.localStorage.setItem=(k,v)=>{stored.push(`${k}=${v}`);};
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    const lineKey=()=>simple('key:alpha');
    const entryKey=selector=>more('ai').querySelectorAll('.provider-entry').find(n=>n.dataset.selector===selector)
      ?.querySelectorAll('input').find(n=>n.dataset.field==='providers.key');
    const buttonBy=input=>input.parentElement.querySelectorAll('button')[0];
    const exactValue=()=>more('privacy').querySelectorAll('input').find(n=>n.type==='password');
    const hashButton=()=>more('privacy').querySelectorAll('button').find(n=>text(n)===ctx.tier.text('redaction_hash_add','en'));
    const secrets=['made-up-line-key-0001','made-up-entry-key-0002','made-up-exact-value-0003'];
    typeIn(lineKey(),secrets[0]);typeIn(entryKey('config:1'),secrets[1]);typeIn(exactValue(),secrets[2]);
    givePrivacy(json(privacy));await settled();
    const kept=()=>[lineKey().value,entryKey('config:1').value,exactValue().value];
    assert.deepEqual(kept(),secrets,'kept through the read\'s answer');
    [privacyAnswer,givePrivacy]=later();
    answers=[json(keyed({version:'v2',inject:sized(4000)}))];
    typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.length,kept()],[1,secrets],'kept through a save\'s answer, which drew the page again');
    ctx.ui.setLang('ja');ctx.ui.drawSettings();
    assert.deepEqual(kept(),secrets,'and through the page drawn in another language');
    ctx.ui.setLang('en');ctx.ui.drawSettings();
    assert.deepEqual([buttonBy(lineKey()).disabled,buttonBy(entryKey('config:1')).disabled,hashButton().disabled],[false,false,false],
      'their buttons enabled');
    const walk=n=>[n,...n.children.filter(c=>c&&typeof c==='object').flatMap(walk)];
    const attributes=()=>JSON.stringify(walk(panel).map(n=>[n.attrs,n.dataset]));
    const seen=JSON.stringify(ctx.ui.getForm())+JSON.stringify(posted)+stored.join('\n')+attributes();
    assert(secrets.every(s=>!seen.includes(s)),'in no form, storage, request or attribute');
    // An entry whose address changed (edited elsewhere) gets nothing back; the others keep theirs.
    [privacyAnswer,givePrivacy]=later();
    const readdressed=keyed({version:'v2b',inject:sized(4050)});
    readdressed.providers[1]={...readdressed.providers[1],saved:{...readdressed.providers[1].saved,base_url:'http://127.0.0.1:9/elsewhere'}};
    answers=[json(readdressed)];
    typeIn(general('inject.session_start_chars'),'4050');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual(kept(),[secrets[0],'',secrets[2]],'nothing put into an entry whose address changed');
    // Or whose API changed: a key typed for one API's server is not offered to another's.
    typeIn(entryKey('config:1'),'made-up-entry-key-0009');
    [privacyAnswer,givePrivacy]=later();
    const reapi=keyed({version:'v2c',inject:sized(4060)});
    reapi.providers[1]={...reapi.providers[1],saved:{...reapi.providers[1].saved,base_url:'http://127.0.0.1:9/elsewhere',api:'anthropic'}};
    answers=[json(reapi)];
    typeIn(general('inject.session_start_chars'),'4060');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.equal(entryKey('config:1').value,'','nothing put into an entry whose API changed');
    // An entry whose place now holds another entry, or whose address changed, gets nothing back:
    // a key never goes where it was not typed for.
    typeIn(entryKey('config:0'),'made-up-entry-key-0004');
    [privacyAnswer,givePrivacy]=later();
    const moved=keyed({version:'v3',inject:sized(4000)});
    moved.providers=[{...moved.providers[1],selector:{source:'config',index:0}},{...moved.providers[0],selector:{source:'config',index:1},
      saved:{...moved.providers[0].saved,base_url:'http://127.0.0.1:9/other'}}];
    answers=[json(moved)];
    typeIn(general('inject.session_start_chars'),'4100');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([lineKey().value,entryKey('config:0').value,entryKey('config:1').value],['','',''],
      'nothing put into a field whose entry moved, changed its address or is another');
    assert.equal(exactValue().value,secrets[2],'the exact value, which goes nowhere, stays');
    // Its own save sends it, and nothing is kept once it is sent.
    typeIn(lineKey(),secrets[0]);
    answers=[json({...keyed({version:'v4',inject:sized(4100)}),key_saved:{durable:true}})];
    buttonBy(lineKey()).click();await settled();
    assert.deepEqual([posted.at(-1).url,JSON.parse(posted.at(-1).body).key],['/api/providers/key',secrets[0]],'its own save sends it');
    assert.equal(lineKey()?.value??'','','and the field is empty after');
    // A key its own save sent is not put back by the redraw a stale answer brings, nor the field
    // refilled after a refusal.
    typeIn(lineKey(),'made-up-stale-key-0010');
    [privacyAnswer,givePrivacy]=later();
    answers=[json({code:'stale'},409)];
    buttonBy(lineKey()).click();await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.at(-1).url,lineKey()?.value??''],['/api/providers/key',''],'not put back after a stale reload');
    typeIn(lineKey(),'made-up-refused-key-0011');
    answers=[json({code:'bad_key',field:'providers.key'},422)];
    buttonBy(lineKey()).click();await settled();
    ctx.ui.drawSettings();
    assert.equal(lineKey()?.value??'','','nor after a refusal and a redraw');
    // Two entries of one name whose address the viewer does not show cannot be told apart: neither
    // keeps a key through the page drawn again, so one typed for the first never reaches the
    // second's field (Codex's security review of #807).
    const unshown=n=>({...entry('alpha',n),saved:{...entry('alpha',n).saved,base_url:null,endpoint_supported:false}});
    const twins=(extra={})=>keyed({chain:[row('alpha',{entries:2})],providers:[unshown(0),unshown(1)],...extra});
    settingsNow=twins();
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    givePrivacy(json(privacy));await settled();
    typeIn(entryKey('config:0'),'made-up-twin-key-0008');
    [privacyAnswer,givePrivacy]=later();
    answers=[json(twins({version:'v6',inject:sized(4000)}))];
    typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.length,entryKey('config:0').value,entryKey('config:1').value],[1,'',''],
      'an entry whose address is not shown keeps no key');
    // The judge's key is kept too, with its destination: another destination chosen, its field is
    // empty. Search's Workers AI key is not kept: its endpoint may be set in config.toml beyond what
    // the page shows (Codex's security review of #807).
    const judged={...judge,destinations:[...judge.destinations,{id:'d1',key:'missing',usd_per_mtok_in:0.01,local:false},
      {id:'clef',key:'missing',usd_per_mtok_in:0.02,local:false}]};
    settingsNow=keyed({judge:judged});
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    givePrivacy(json(privacy));await settled();
    const others=['made-up-workers-key-0005','made-up-judge-key-0006'];
    typeIn(general('embedding.key'),others[0]);typeIn(general('judge.key'),others[1]);
    [privacyAnswer,givePrivacy]=later();
    answers=[json(keyed({judge:judged,version:'v5',inject:sized(4000)}))];
    typeIn(general('inject.session_start_chars'),'4000');leave(general('inject.session_start_chars'));await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.length,general('embedding.key').value,general('judge.key').value],[1,'',others[1]],'the judge\'s key is kept, search\'s is not');
    const to=general('judge.destination');to.value='d1';for(const cb of to.listeners.change)cb({target:to});await settled();
    assert.equal(general('judge.key').value,'','a judge key typed for one destination is not put into another\'s field');
    // Cloudflare's: kept for the account typed beside it, not put back once another is typed (cubic on #811).
    to.value='clef';for(const cb of to.listeners.change)cb({target:to});await settled();
    typeIn(general('judge.account_id'),'0123456789abcdef0123456789abcdef');ctx.ui.drawSettings();
    typeIn(general('judge.key'),'made-up-cloudflare-key-0012');ctx.ui.drawSettings();
    assert.equal(general('judge.key').value,'made-up-cloudflare-key-0012','a Cloudflare key is kept for its account');
    typeIn(general('judge.account_id'),'fedcba9876543210fedcba9876543210');ctx.ui.drawSettings();
    assert.equal(general('judge.key').value,'','not put back once another account is typed');
    // Named by the account typed when the key is typed, not the one its field was drawn with
    // (Codex and CodeRabbit on #811).
    typeIn(general('judge.account_id'),'00112233445566778899aabbccddeeff');
    typeIn(general('judge.key'),'made-up-cloudflare-key-0034');ctx.ui.drawSettings();
    assert.equal(general('judge.key').value,'made-up-cloudflare-key-0034','a key typed after its account, no redraw between, is kept for it');
    typeIn(general('judge.account_id'),'ffeeddccbbaa99887766554433221100');
    typeIn(general('judge.key'),'made-up-cloudflare-key-0056');
    typeIn(general('judge.account_id'),'00112233445566778899aabbccddeeff');ctx.ui.drawSettings();
    assert.equal(general('judge.key').value,'','a key typed for another account is not put back for the one its field was drawn with');
    others.push('made-up-cloudflare-key-0034','made-up-cloudflare-key-0056');
    assert(others.every(s=>!(JSON.stringify(ctx.ui.getForm())+JSON.stringify(posted)+stored.join('\n')).includes(s)),'in no form, storage or request');
    ctx.localStorage.setItem=setItem;
    // #810: a privacy action pressed from the keyboard gives the focus back once its part is drawn
    // again in place: Refresh to Refresh, an exclusion to its row's button, a place undone whose
    // row left its list to the folder field; not when the focus went elsewhere meanwhile. A browser
    // takes the focus off a control disabled or made inert under it: `blur` does so here.
    const blur=()=>{document.activeElement=document.body;};
    settingsNow=ai(['alpha']);
    [privacyAnswer,givePrivacy]=later();
    (await ctx.ui.showSettings())();posted.length=0;
    givePrivacy(json(privacy));await settled();
    [privacyAnswer,givePrivacy]=later();
    refreshButton().focus();refreshButton().click();blur();givePrivacy(json(privacy));await settled();
    assert.equal(document.activeElement,refreshButton(),'Refresh has the focus back, drawn again');
    [privacyAnswer,givePrivacy]=later();
    refreshButton().focus();refreshButton().click();blur();simple('never.folder').focus();givePrivacy(json(privacy));await settled();
    assert.equal(document.activeElement,simple('never.folder'),'the focus moved meanwhile stays where it went');
    [privacyAnswer,givePrivacy]=later();
    answers=[json({ok:true})];
    simple('exclude:r2').focus();simple('exclude:r2').click();blur();await settled();
    givePrivacy(json({...privacy,repositories:privacy.repositories.map(r=>r.selector==='r2'?{...r,excluded:true}:r)}));await settled();
    assert.deepEqual([posted.at(-1).url,document.activeElement===simple('exclude:r2')],['/api/privacy/exclude',true],
      'an exclusion has it back on its row, drawn again');
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:false})];
    simple('never:repo:r1').focus();simple('never:repo:r1').click();blur();await settled();
    givePrivacy(json({...privacy,repositories:privacy.repositories.map(r=>({...r,never_recorded:false}))}));await settled();
    assert.deepEqual([posted.at(-1).url,simple('never:repo:r1'),Boolean(simple('never-more:repo:r1')),document.activeElement===simple('never.folder')],
      ['/api/privacy/capture',undefined,true,true],'a place undone, whose row moved to the list in More settings, gives it to the folder field');
    // A folder added by Enter in its field gives the focus back to the field, emptied for the next.
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:true})];
    simple('never.folder').focus();typeIn(simple('never.folder'),'made-up-folder');
    for(const cb of simple('never.folder').listeners.keydown)cb({key:'Enter',target:simple('never.folder'),preventDefault(){}});
    blur();await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([posted.at(-1).url,document.activeElement===simple('never.folder'),simple('never.folder').value],['/api/privacy/capture',true,''],
      'a folder added by Enter gives the focus back to its field');
    // The focus moved meanwhile to a control with no name in a part drawn again (the places' fold
    // heading) stays on it, at its place, and Refresh takes nothing (Codex's review of #810).
    const neverBlock=()=>panel.querySelector('.never-record');
    [privacyAnswer,givePrivacy]=later();
    refreshButton().focus();refreshButton().click();blur();
    neverBlock().querySelectorAll('summary')[0].focus();
    givePrivacy(json(privacy));await settled();
    assert(document.activeElement===neverBlock().querySelectorAll('summary')[0]&&document.activeElement!==refreshButton(),
      'a fold heading the focus moved to keeps it, drawn again');
    // So do the fold's heading and the link after it, when the read changes how many rows come
    // before them (Codex's review of #810).
    const twoPlaces={...privacy,folders:[{label:'/home/made-up-user/kept-folder',selector:'f9'}]};
    [privacyAnswer,givePrivacy]=later();
    refreshButton().focus();refreshButton().click();blur();
    simple('link:simple_never_more:').focus();
    givePrivacy(json(twoPlaces));await settled();
    assert.equal(document.activeElement,simple('link:simple_never_more:'),'the link the focus moved to keeps it, the rows before it grown');
    [privacyAnswer,givePrivacy]=later();
    refreshButton().focus();refreshButton().click();blur();
    neverBlock().querySelectorAll('summary')[0].focus();
    givePrivacy(json(privacy));await settled();
    assert.equal(document.activeElement,neverBlock().querySelectorAll('summary')[0],'and the fold\'s heading, the rows shrunk');
    // The folder's own button gives it back to itself; a place whose row left the list, to the folder
    // field; in More settings, to the pattern's button.
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:true})];
    typeIn(simple('never.folder'),'/home/made-up-user/made-up-folder');
    simple('never.folder.add').focus();simple('never.folder.add').click();blur();await settled();
    givePrivacy(json(privacy));await settled();
    assert.equal(document.activeElement,simple('never.folder.add'),'the folder\'s button has it back');
    const patterned={...privacy,patterns:[{label:'made-up-pattern-*',selector:'p1'}]};
    [privacyAnswer,givePrivacy]=later();
    refreshButton().click();givePrivacy(json(patterned));await settled();
    [privacyAnswer,givePrivacy]=later();
    answers=[json({})];
    simple('never:pattern:p1').focus();simple('never:pattern:p1').click();blur();await settled();
    givePrivacy(json(privacy));await settled();
    assert.deepEqual([simple('never:pattern:p1'),document.activeElement===simple('never.folder')],[undefined,true],
      'a pattern undone, its row gone, gives it to the folder field');
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:true})];
    simple('never-more:repo:r2').focus();simple('never-more:repo:r2').click();blur();await settled();
    givePrivacy(json({...privacy,repositories:privacy.repositories.map(r=>({...r,never_recorded:true}))}));await settled();
    assert.deepEqual([simple('never-more:repo:r2'),document.activeElement===simple('never.pattern.add')],[undefined,true],
      'a repository no longer recorded, its row gone from More settings, gives it to the pattern\'s button');
    // A write whose read after it fails draws no field to give the focus to: Refresh takes it.
    [privacyAnswer,givePrivacy]=later();
    answers=[json({never_recorded:true})];
    simple('never.folder').focus();typeIn(simple('never.folder'),'/home/made-up-user/another-folder');
    for(const cb of simple('never.folder').listeners.keydown)cb({key:'Enter',target:simple('never.folder'),preventDefault(){}});
    blur();await settled();
    givePrivacy(json({},503));await settled();
    assert.deepEqual([simple('never.folder'),document.activeElement===refreshButton()],[undefined,true],'Refresh takes it when the state is not read again');
    document.activeElement=null;
  }
  // 69b-3c-1a: each AI line's calls a day, timeout and model are saved by leaving the field or Enter,
  // one change in `[chain]`, without holding the page; what is typed meanwhile stays; a form read
  // afresh and a name gone drop the drafts.
  {
    let reread=null;
    ctx.fetch=(url,options)=>{
      if(options?.method==='POST'){posted.push({url,body:options.body});return Promise.resolve(answers.shift());}
      if(url.startsWith('/api/settings?')&&reread)return Promise.resolve(json(reread));
      if(url.startsWith('/api/hidden?'))return Promise.resolve(json([]));
      return new Promise(()=>{});
    };
    const NOCAP=4294967295;
    const lineAi=(rows,extra={})=>ai(rows.map(r=>r.name),{ranges:{...base.ranges,daily_requests:[1,100000],no_cap:NOCAP},chain:rows,...extra});
    const one=(extra={},more={})=>lineAi([row('alpha',extra)],more);
    const leaveLine=node=>{for(const cb of node.listeners.blur||[])cb({target:node,relatedTarget:null});};
    const enterLine=node=>{for(const cb of node.listeners.keydown||[])cb({key:'Enter',target:node,isComposing:false,keyCode:13,preventDefault(){}});};
    const calls=(name='alpha')=>simple(`budget:${name}`);
    const timeout=(name='alpha')=>simple(`timeout:${name}`);
    const lineOff=(name='alpha')=>simple(`budget-off:${name}`);
    const lineOf=(name='alpha')=>all('.simple-provider').find(n=>n.dataset.providerName===name);
    const warned=()=>!lineOf().querySelector('.no-cap-warning').hidden;
    const edit=()=>JSON.parse(JSON.stringify(ctx.ui.getForm().chain.find(r=>r.name==='alpha').edit));
    const freshEdit=s=>JSON.parse(JSON.stringify(w6.formOf(s).chain.find(r=>r.name==='alpha').edit));
    const wholeChain=()=>{const r=w6.saveBody(true).body.chain[0];return [r.daily_budget,r.timeout_s,r.model];};
    // In this realm's arrays, as the expected values are.
    const drafts=()=>[...vm.runInContext('[...lineDrafts.keys()]',ctx)];
    const inert=()=>Boolean(panel.querySelector('.settings').inert);
    // Saved on leaving the field, one change in `[chain]`; the row's edit then is a fresh load's, and
    // the next general save sends what is saved: a number, back to the entries' own, No cap.
    f=draw(one());
    typeIn(calls(),'20');await settled();
    assert.equal(posted.length,0,'typing sends nothing');
    let answer=one({daily_budget:20,effective_daily_budget:20},{version:'v2'});
    answers=[json(answer)];
    leaveLine(calls());await settled();
    assert.deepEqual(sent().map(b=>b.chain),[[{name:'alpha',on:true,daily_budget:20,timeout_s:null,model:null}]],'leaving it saves one change in [chain]');
    assert.deepEqual([edit(),wholeChain(),calls().value,drafts()],[freshEdit(answer),[20,null,null],'20',[]],
      'its row as a fresh load makes it, a general save sends it, and no draft is left');
    answers=[json(one({daily_budget:20,effective_daily_budget:20},{version:'v3',summary:{...base.summary,idle_minutes:11}}))];
    typeIn(general('summary.idle_minutes'),'11');leave(general('summary.idle_minutes'));await settled();
    assert.equal(sent()[1].chain[0].daily_budget,20,'a general save after the line\'s answer sends the line\'s value');
    f=draw(one({daily_budget:20,effective_daily_budget:20}));
    answer=one({},{version:'v2'});answers=[json(answer)];
    calls().focus();typeIn(calls(),'');enterLine(calls());await settled();
    assert.equal(document.activeElement,calls(),'after Enter the field has the focus back');
    leaveLine(calls());await settled();
    assert.deepEqual([posted.length,sent()[0].chain[0].daily_budget,edit(),wholeChain()[0]],[1,null,freshEdit(answer),null],
      'emptied, Enter sends null once, the blur after it nothing');
    f=draw(one());
    answer=one({daily_budget:NOCAP,effective_daily_budget:null},{version:'v2'});answers=[json(answer)];
    assert.deepEqual([lineOff().checked,warned()],[false,false],'a cap in effect: No cap off, no warning');
    flip(lineOff(),true);await settled();
    assert.deepEqual([sent()[0].chain[0].daily_budget,lineOff().checked,calls().disabled,warned(),edit(),wholeChain()[0]],
      [NOCAP,true,true,true,freshEdit(answer),NOCAP],'No cap on sends no_cap, its warning shown, the field set aside');
    // The Enter that ends an input method's composition saves nothing (a model name typed in Japanese
    // input would be saved half converted).
    f=draw(one());
    pickModel('__other');typeIn(typedModel(),'some-model');
    for(const composing of [{isComposing:true},{keyCode:229}])
      for(const cb of typedModel().listeners.keydown)cb({key:'Enter',target:typedModel(),preventDefault(){},...composing});
    typeIn(calls(),'20');
    for(const cb of calls().listeners.keydown)cb({key:'Enter',target:calls(),preventDefault(){},isComposing:true});
    await settled();
    assert.equal(posted.length,0,'an Enter ending a composition saves nothing');
    // Invalid or unchanged sends nothing.
    f=draw(one());
    typeIn(calls(),'0');leaveLine(calls());typeIn(timeout(),'');leaveLine(timeout());await settled();
    assert.deepEqual([posted.length,calls().attrs['aria-invalid'],drafts().length],[0,'true',1],
      'out of range is marked and sent not; the saved value sends nothing and keeps no draft');
    // No cap turned off with no number, the entries' own having none: the field opens, focused,
    // nothing sent; through another answer and a language change it stays open; a number left is
    // saved; left empty, by Tab or Enter, it stays open, nothing sent; the switch turns No cap on
    // again, nothing sent.
    const unlimited=()=>one({daily_budget:null,effective_daily_budget:null});
    f=draw(unlimited());
    flip(lineOff(),false);await settled();
    assert.deepEqual([posted.length,calls().disabled,lineOff().checked,warned(),document.activeElement===calls()],[0,false,false,false,true],
      'turned off with no number, the field opens for one');
    answers=[json(lineAi([row('alpha',{daily_budget:null,effective_daily_budget:null,timeout_s:120,effective_timeout_s:120})],{version:'v2'}))];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    ctx.ui.setLang('ja');ctx.ui.drawSettings();ctx.ui.setLang('en');ctx.ui.drawSettings();
    assert.deepEqual([posted.length,calls().disabled,lineOff().checked],[1,false,false],'it stays open through another answer and a language change');
    answers=[json(lineAi([row('alpha',{daily_budget:30,effective_daily_budget:30,timeout_s:120,effective_timeout_s:120})],{version:'v3'}))];
    typeIn(calls(),'30');leaveLine(calls());await settled();
    assert.deepEqual([sent()[1].chain[0].daily_budget,lineOff().checked,calls().value],[30,false,'30'],'a number left in it is saved');
    for(const how of ['left','Enter']){
      f=draw(unlimited());
      flip(lineOff(),false);await settled();
      if(how==='left')leaveLine(calls());else enterLine(calls());
      await settled();
      assert.deepEqual([posted.length,lineOff().checked,calls().disabled,drafts().length],[0,false,false,1],`${how} empty, it stays open, nothing sent`);
    }
    flip(lineOff(),true);await settled();
    assert.deepEqual([posted.length,lineOff().checked,calls().disabled,drafts(),document.activeElement===lineOff()],[0,true,true,[],true],
      'the switch turns No cap on again, nothing sent, the focus on it');
    // `[chain]` lifting the entries' calls a day: turned off with no number, it sets none again, back
    // to the entries' own, and the field opens, the answer's value in effect as its placeholder.
    f=draw(one({daily_budget:NOCAP,effective_daily_budget:null}));
    answers=[json(one({daily_budget:null,effective_daily_budget:200},{version:'v2'}))];
    flip(lineOff(),false);await settled();
    assert.deepEqual([sent().map(b=>b.chain[0].daily_budget),lineOff().checked,calls().disabled,calls().value,calls().placeholder],
      [[null],false,false,'','200'],'turned off, [chain] sets none: the entries\' own 200 is in effect, the field open');
    // The entries' own having none, the answer leaves No cap in effect: the field stays open for a
    // number, the switch off, the focus on the field.
    f=draw(one({daily_budget:NOCAP,effective_daily_budget:null}));
    answers=[json(one({daily_budget:null,effective_daily_budget:null},{version:'v2'}))];
    flip(lineOff(),false);await settled();
    assert.deepEqual([sent().map(b=>b.chain[0].daily_budget),lineOff().checked,calls().disabled,document.activeElement===calls()],
      [[null],false,false,true],'with none of their own, the field stays open for a number after the answer');
    let give;
    // The switch acts on the requests as they are when it is pressed: the last change made is what
    // is saved (Codex's review of 69b-3c-1a). Off, a number asked, then on again before the answers.
    f=draw(one({daily_budget:NOCAP,effective_daily_budget:null}));
    [answer,give]=later();answers=[answer,json(one({daily_budget:NOCAP,effective_daily_budget:null},{version:'v3'}))];
    flip(lineOff(),false);await settled();
    typeIn(calls(),'30');enterLine(calls());await settled();
    flip(lineOff(),true);await settled();
    give(json(one({daily_budget:null,effective_daily_budget:null},{version:'v2'})));await settled();
    assert.deepEqual([sent().map(b=>b.chain[0].daily_budget),lineOff().checked,drafts()],[[null,NOCAP],true,[]],
      'off, 30 asked, then on: 30 is never sent, No cap is saved last');
    // On asked, then off before its answer: the number shown is saved after it.
    f=draw(one({daily_budget:50,effective_daily_budget:50}));
    [answer,give]=later();answers=[answer,json(one({daily_budget:50,effective_daily_budget:50},{version:'v3'}))];
    flip(lineOff(),true);await settled();
    flip(lineOff(),false);await settled();
    give(json(one({daily_budget:NOCAP,effective_daily_budget:null},{version:'v2'})));await settled();
    assert.deepEqual([sent().map(b=>b.chain[0].daily_budget),lineOff().checked,calls().value],[[NOCAP,50],false,'50'],
      'on, then off before its answer: 50 is saved after it');
    // On asked, then off with no number before its answer: `[chain]` sets none after it, and the
    // field stays open through both answers.
    f=draw(one({daily_budget:null,effective_daily_budget:200}));
    [answer,give]=later();answers=[answer,json(one({daily_budget:null,effective_daily_budget:200},{version:'v3'}))];
    flip(lineOff(),true);await settled();
    flip(lineOff(),false);await settled();
    give(json(one({daily_budget:NOCAP,effective_daily_budget:null},{version:'v2'})));await settled();
    assert.deepEqual([sent().map(b=>b.chain[0].daily_budget),lineOff().checked,calls().disabled,calls().placeholder],[[NOCAP,null],false,false,'200'],
      'on, then off with no number before its answer: back to the entries\' own after it, the field open');
    // A line's save holds nothing: the page is usable, the field enabled, and what is typed in
    // another field meanwhile stays, unsent, with its caret, through the answer and a language
    // change; a redraw's blur is no leaving. Left, it is saved on what the answer saved.
    f=draw(one());
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    assert.deepEqual([posted.length,inert(),timeout().disabled],[1,false,false],'the page is not held while a line saves');
    calls().focus();typeIn(calls(),'2');
    give(json(one({timeout_s:120,effective_timeout_s:120},{version:'v2'})));await settled();
    assert.deepEqual([posted.length,calls().value,document.activeElement===calls()],[1,'2',true],
      'a draft typed during another save\'s answer stays unsent and focused');
    ctx.ui.setLang('ja');ctx.ui.drawSettings();ctx.ui.setLang('en');ctx.ui.drawSettings();
    assert.deepEqual([posted.length,calls().value],[1,'2'],'and through a language change');
    answers=[json(one({daily_budget:2,effective_daily_budget:2,timeout_s:120,effective_timeout_s:120},{version:'v3'}))];
    leaveLine(calls());await settled();
    assert.deepEqual([sent()[1].version,sent()[1].chain[0].daily_budget,sent()[1].chain[0].timeout_s],['v2',2,120],'left, it is saved on the answer\'s values');
    // A general field typed in during a line's save is not saved by the redraw of its answer, only
    // when it is left (Chrome runs a focusout there too).
    f=draw(one());
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    const size=general('inject.session_start_chars');
    size.focus();typeIn(size,'4');fire('input',{target:size});
    give(json(one({timeout_s:120,effective_timeout_s:120},{version:'v2'})));await settled();
    assert.deepEqual([posted.length,general('inject.session_start_chars').value,document.activeElement===general('inject.session_start_chars')],[1,'4',true],
      'a general field being typed is not saved by the answer\'s redraw');
    answers=[json(one({timeout_s:120,effective_timeout_s:120},{version:'v3',inject:sized(4000)}))];
    typeIn(general('inject.session_start_chars'),'4000');fire('input',{target:general('inject.session_start_chars')});
    leave(general('inject.session_start_chars'));await settled();
    assert.deepEqual([posted.length,sent()[1].inject.session_start_chars],[2,4000],'left, it is saved');
    // Enter then blur, and a blur after a 409, a refusal or a failure: one POST each; a 409 reloads
    // and drops the draft; Enter again sends again.
    for(const outcome of ['saved','stale','refused','failed']){
      f=draw(one());
      reread=one({},{version:'v9'});
      answers=[{saved:()=>json(one({daily_budget:25,effective_daily_budget:25},{version:'v2'})),stale:()=>json({code:'stale'},409),
        refused:()=>json({code:'bad_request',field:'chain.alpha.daily_budget'},400),failed:()=>Promise.reject(new Error('down'))}[outcome]()];
      typeIn(calls(),'25');enterLine(calls());await settled();leaveLine(calls());await settled();
      assert.equal(posted.length,1,`${outcome}: Enter then the blur send one POST`);
      if(outcome==='stale')assert.deepEqual([calls().value,drafts()],['',[]],'a stale reload drops the draft');
      if(outcome==='refused'){
        assert.equal(calls().attrs['aria-invalid'],'true','the refused field is marked');
        answers=[json(one({daily_budget:25,effective_daily_budget:25},{version:'v2'}))];
        enterLine(calls());await settled();
        assert.equal(posted.length,2,'Enter again sends again');
      }
    }
    reread=null;
    // Typed back before the answer: what is typed stays, unsent, and its blur saves it; asked while
    // another save waits, then typed back to the saved value, the older value is never sent.
    for(const back of ['10','30']){
      f=draw(one({daily_budget:10,effective_daily_budget:10}));
      [answer,give]=later();answers=[answer];
      typeIn(calls(),'20');enterLine(calls());await settled();typeIn(calls(),back);
      give(json(one({daily_budget:20,effective_daily_budget:20},{version:'v2'})));await settled();
      assert.deepEqual([posted.length,calls().value],[1,back],`typed back to ${back} before the answer, it stays unsent`);
      answers=[json(one({daily_budget:Number(back),effective_daily_budget:Number(back)},{version:'v3'}))];
      leaveLine(calls());await settled();
      assert.deepEqual([posted.length,sent()[1].chain[0].daily_budget],[2,Number(back)],'its blur saves it');
    }
    f=draw(one({daily_budget:10,effective_daily_budget:10}));
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    typeIn(calls(),'20');enterLine(calls());typeIn(calls(),'10');leaveLine(calls());await settled();
    give(json(one({daily_budget:10,effective_daily_budget:10,timeout_s:120,effective_timeout_s:120},{version:'v2'})));await settled();
    assert.deepEqual([posted.length,calls().value,drafts()],[1,'10',[]],'20 asked behind another save, then 10 left: 20 is never sent, 10 stays saved');
    // While 20 is on its way, typed back to the 10 saved before it and left: 10 is sent after it.
    f=draw(one({daily_budget:10,effective_daily_budget:10}));
    [answer,give]=later();answers=[answer];
    typeIn(calls(),'20');enterLine(calls());await settled();
    typeIn(calls(),'10');leaveLine(calls());await settled();
    answers=[json(one({daily_budget:10,effective_daily_budget:10},{version:'v3'}))];
    give(json(one({daily_budget:20,effective_daily_budget:20},{version:'v2'})));await settled();
    assert.deepEqual([posted.length,sent()[1]?.version,sent()[1]?.chain[0].daily_budget],[2,'v2',10],
      'typed back to the value saved before and left while 20 is on its way: 10 is sent after it');
    // A request asked before its draft was dropped never passes for one asked after (requests are
    // numbered across drafts).
    f=draw(one());
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    typeIn(calls(),'20');enterLine(calls());await settled();
    reread=one({},{version:'v5'});
    (await ctx.ui.showSettings())();await settled();
    answers=[json(one({daily_budget:30,effective_daily_budget:30},{version:'v6'}))];
    typeIn(calls(),'30');enterLine(calls());await settled();
    give(json(one({timeout_s:120,effective_timeout_s:120},{version:'v2'})));await settled();
    assert.deepEqual(sent().map(b=>b.chain[0].daily_budget),[null,30],'the 20 asked before Settings was entered again is never sent');
    reread=null;
    // A switch pressed alone keeps no text: a redraw before its answer shows the field as saved.
    f=draw(one({daily_budget:20,effective_daily_budget:20}));
    [answer,give]=later();answers=[answer];
    flip(lineOff(),true);await settled();
    ctx.ui.setLang('ja');ctx.ui.drawSettings();ctx.ui.setLang('en');ctx.ui.drawSettings();
    assert.equal(calls().value,'20','a switch pressed alone leaves the field as saved through a redraw');
    give(json(one({daily_budget:NOCAP,effective_daily_budget:null},{version:'v2'})));await settled();
    // Entering Settings and a name gone drop the drafts.
    f=draw(lineAi([row('alpha'),row('beta')]));
    typeIn(calls('beta'),'44');typeIn(calls(),'33');
    reread=lineAi([row('alpha'),row('beta')]);
    (await ctx.ui.showSettings())();await settled();
    assert.deepEqual([drafts(),calls().value,calls('beta').value],[[],'',''],'entering Settings drops them');
    typeIn(calls('beta'),'44');
    answers=[json(lineAi([row('alpha',{timeout_s:120,effective_timeout_s:120})],{version:'v2'}))];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    assert.deepEqual([drafts(),calls('beta')],[[],undefined],'a name gone takes its draft');
    reread=null;
    // A subscription says its calls a day and has the switch only while an entry of its name is
    // capped (#800 item 3); several entries are said.
    for(const [extra,said,switched] of [[{capped:true,effective_daily_budget:50},'Calls a day: 50',true],
      [{capped:true,effective_daily_budget:null,differs:['daily_budget']},'Calls a day: its entries differ',true],
      [{capped:false,effective_daily_budget:null},null,false]]){
      f=draw(one({subscription:true,entries:2,...extra}));
      assert.deepEqual([calls(),Boolean(lineOff()),said===null||text(lineOf()).includes(said)],[undefined,switched,true],
        `a subscription ${switched?'capped':'not capped'}: no field, ${switched?'its switch':'no switch'}`);
      assert(text(lineOf()).includes('These settings apply to all 2 entries of this name.'),'its entries are said');
    }
    // A subscription lifted by `[chain]`: its switch shows on, and off sets none, back to its entries'.
    f=draw(one({subscription:true,daily_budget:NOCAP,effective_daily_budget:null,capped:false}));
    assert.equal(lineOff()?.checked,true,'a subscription lifted by [chain] shows its switch on');
    answers=[json(one({subscription:true,daily_budget:null,effective_daily_budget:50,capped:true},{version:'v2'}))];
    flip(lineOff(),false);await settled();
    assert.deepEqual([sent()[0].chain[0].daily_budget,lineOff()?.checked,text(lineOf()).includes('Calls a day: 50')],[null,false,true],
      'off sets none, back to its entries\' 50');
    // Calls a day and a timeout its entries differ in, none set for the name, are said, not shown as
    // No cap or as the first entry's (Codex's review of 69b-3c-1a).
    f=draw(one({entries:2,differs:['daily_budget','timeout_s'],effective_daily_budget:null}));
    assert.deepEqual([lineOff().checked,calls().disabled,calls().placeholder,timeout().placeholder,text(lineOf()).includes('Calls a day: its entries differ')],
      [false,false,'','',true],'calls a day and timeout its entries differ in are said, not shown as one');
    // The model: another saved to `[chain]` by Enter; a free rule's needs :free; a fixed rule's
    // model set for its name removed.
    f=draw(one());
    answers=[json(one({model:'other-model',effective_model:'other-model'},{version:'v2'}))];
    pickModel('__other');typeIn(typedModel(),'other-model');enterLine(typedModel());await settled();
    assert.equal(sent()[0].chain[0].model,'other-model','another model is saved to [chain]');
    assert.deepEqual([typedModel(),document.activeElement===simple('model:alpha')],[undefined,true],
      'the model name saved by Enter closes its field, the focus on the model choice');
    // Another model opened while a model chosen is on its way stays open through its answer.
    f=draw(one());
    [answer,give]=later();answers=[answer];
    pickModel('model-alpha2');await settled();
    pickModel('__other');
    give(json(one({model:'model-alpha2',effective_model:'model-alpha2'},{version:'v2'})));await settled();
    assert.deepEqual([posted.length,Boolean(typedModel())],[1,true],'another model opened meanwhile stays open');
    f=draw(one({model_rule:'free'}));
    pickModel('__other');typeIn(typedModel(),'some-model');leaveLine(typedModel());await settled();
    assert.deepEqual([posted.length,typedModel().attrs['aria-invalid']],[0,'true'],'a free rule\'s model without :free is marked, not sent');
    f=draw(one({model_rule:'fixed',model:'old-model',model_applied:false}));
    assert(text(lineOf()).includes('The model set for this name, old-model, is not used by every entry of it.'),'a model not every entry takes is said');
    answers=[json(one({model_rule:'fixed'},{version:'v2'}))];
    simple('model-remove:alpha').click();await settled();
    assert.equal(sent()[0].chain[0].model,null,'and removed');
    f=draw(one({model:'over-model',model_applied:false}));
    answers=[json(one({},{version:'v2'}))];
    simple('model-remove:alpha').click();await settled();
    assert.equal(sent()[0].chain[0].model,null,'an any rule\'s model set for its name has the button too');
    // Typed on after an Enter that changed nothing, another model's field stays open with its text.
    f=draw(one({model:'pinned-model',effective_model:'pinned-model'}));
    pickModel('__other');enterLine(typedModel());await settled();
    typeIn(typedModel(),'pinned-model-2');ctx.ui.drawSettings();
    assert.deepEqual([posted.length,typedModel()?.value],[0,'pinned-model-2'],'typed on after an Enter that changed nothing, it stays open');
    // An AI the guide's Apply turns on or off in the form keeps that through a line's answer, its own
    // line's included, and the general save after it sends it (Codex's review of 69b-3c-1a).
    f=draw(lineAi([row('alpha'),row('beta')]));
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    for(const r of ctx.ui.getForm().chain)r.edit.on=false;
    give(json(lineAi([row('alpha',{timeout_s:120,effective_timeout_s:120}),row('beta')],{version:'v2'})));await settled();
    assert.deepEqual(JSON.parse(JSON.stringify(w6.saveBody(true).body.chain.map(r=>[r.name,r.on]))),[['alpha',false],['beta',false]],
      'the AI turned off in the form stay off through a line\'s answer');
    // A judge's key saved while a line's save is on its way waits for it, then sends with the version
    // its answer gave.
    f=draw(one({},{judge,key_input:true}));
    [answer,give]=later();answers=[answer,json({...one({},{version:'v3',judge,key_input:true}),key_saved:{durable:true}})];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    const judgeKey=general('judge.key');
    typeIn(judgeKey,'made-up-judge-key-0078');judgeKey.parentElement.querySelectorAll('button')[0].click();await settled();
    assert.equal(posted.length,1,'the judge\'s key waits for the line\'s save on its way');
    give(json(one({timeout_s:120,effective_timeout_s:120},{version:'v2',judge,key_input:true})));await settled();
    assert.deepEqual([posted.length,posted[1]?.url,JSON.parse(posted[1]?.body??'{}').version],[2,'/api/judge/key','v2'],
      'then it is sent with the version the line\'s answer gave');
    // The page drawn again by that answer is held while the key is on its way, and its field is marked
    // when the key is refused.
    f=draw(one({},{judge,key_input:true}));
    [answer,give]=later();const [keyAnswer,giveKey]=later();answers=[answer,keyAnswer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    typeIn(general('judge.key'),'made-up-judge-key-0079');general('judge.key').parentElement.querySelectorAll('button')[0].click();await settled();
    give(json(one({timeout_s:120,effective_timeout_s:120},{version:'v2',judge,key_input:true})));await settled();
    assert.deepEqual([posted.length,inert()],[2,true],'the page drawn again is held while the key is on its way');
    giveKey(json({code:'bad_key',field:'judge.key'},422));await settled();
    assert.deepEqual([general('judge.key').attrs['aria-invalid'],inert()],['true',false],'a refused key is marked on the field shown');
    // A key waiting across a form read afresh (the line's save stale) is not sent: it is typed again.
    f=draw(one({},{judge,key_input:true}));
    reread=one({},{version:'v9',judge,key_input:true});
    [answer,give]=later();answers=[answer];
    typeIn(timeout(),'120');enterLine(timeout());await settled();
    typeIn(general('judge.key'),'made-up-judge-key-0080');general('judge.key').parentElement.querySelectorAll('button')[0].click();await settled();
    give(json({code:'stale'},409));await settled(6);
    assert.deepEqual([posted.length,document.getElementById('status').textContent.includes('Type it again')],[1,true],
      'a key waiting across a stale reload is not sent, and is to be typed again');
    reread=null;
    vm.runInContext('lineDrafts.clear()',ctx);document.activeElement=null;
  }
  ctx.fetch=w6fetch;
  document.querySelectorAll=pageControls;
  ctx.ui.setForm(null);
  console.log('PASS: decision 69 (69b-2a): one page, the saves in turn, drafts and confirmations kept, focus, the new home\'s resident choice');
}
}

// The owner, 2026-10-09: an HTTP entry's API, chosen in its editor and saved as `api`.
{
vm.runInContext('globalThis.apiUi={providerEdit,providerBody,providerEditor};',ctx);
const {apiUi}=ctx;
ctx.ui.setForm({ranges:{timeout_s:[1,600],daily_budget:[1,1000]}});
ctx.ui.setLang('en');
const draft=apiUi.providerEdit('a',{kind:'openai',base_url:'https://example.invalid/v1',model:'m',timeout_s:30,limits:{}});
assert.equal(draft.api,'openai','an entry saved without an API is OpenAI-compatible');
let editor=apiUi.providerEditor(draft,null,new Node('article'));
const select=editor.querySelectorAll('select').find(node=>node.dataset.field==='providers.api');
assert(select,'the HTTP editor offers the API');
assert.deepEqual(select.options.map(node=>node.value),['openai','anthropic']);
assert.equal(select.value,'openai');
assert(text(editor).includes('Anthropic Messages')&&text(editor).includes('OpenAI-compatible'),'both APIs are named');
const url=editor.querySelectorAll('input').find(node=>node.dataset.field==='providers.base_url');
const capShown=()=>editor.querySelectorAll('input').some(node=>node.dataset.field==='providers.limits.max_output_tokens');
assert(!capShown(),'an unpriced OpenAI-compatible entry has no output cap to set');
url.value='https://api.anthropic.com/v1';for(const cb of url.listeners.input)cb({target:url});
assert.equal(draft.api,'anthropic','an endpoint on the Anthropic host preselects its API');
assert(capShown(),'a Messages entry always sends its output cap, so it can be set');
assert.equal(select.value,'anthropic');
assert.equal(apiUi.providerBody(draft).api,'anthropic','a save sends the choice');
select.value='openai';for(const cb of select.listeners.change)cb({target:select});
assert(!capShown(),'switching back hides the cap again');
url.value='https://api.anthropic.com/v1/';for(const cb of url.listeners.input)cb({target:url});
assert.equal(apiUi.providerBody(draft).api,'openai','editing the URL on the same host keeps a manual choice');
url.value='https://example.invalid/v1';for(const cb of url.listeners.input)cb({target:url});
assert.equal(apiUi.providerBody(draft).api,'openai','the user may still choose the other API');
assert.equal(apiUi.providerEdit('b',{kind:'openai',api:'anthropic',limits:{}}).api,'anthropic','a saved choice is kept');
const cli=apiUi.providerEdit('c',{kind:'cli',cli:'claude',limits:{}});
assert.equal(apiUi.providerBody(cli).api,undefined,'a CLI entry sends no API');
// An AI added from the simple view's catalog keeps its built-in's calls a day (config.rs's
// presets: Gemini's 30 keeps a paid key within the monthly cap), not the generic default.
for(const [name,cap] of [['gemini',30],['groq',200],['groq-20b',200],['groq-qwen',200],['nim',500]]){
  const entry=vm.runInContext('AI_CATALOG',ctx).find(e=>e.name===name);
  assert.equal(apiUi.providerBody(apiUi.providerEdit(entry.name,{...entry,model:entry.models[0]})).daily_budget,cap,`${name} keeps its cap`);
}
assert.equal(apiUi.providerEditor(cli,null,new Node('article')).querySelectorAll('select').length,0);
ctx.ui.setLang('ja');
editor=apiUi.providerEditor(apiUi.providerEdit('a',{kind:'openai',limits:{}}),null,new Node('article'));
assert(text(editor).includes('API の種類')&&text(editor).includes('OpenAI 互換'),'Japanese API choice');
ctx.ui.setLang('en');
console.log('PASS: an HTTP entry API choice');
}
// Owner decision 41 (#164): a codex entry may draw on the account's credits past its plan's limits.
{
const {apiUi}=ctx;
const codex=apiUi.providerEdit('x',{kind:'cli',cli:'codex',credits:true,limits:{}});
assert.equal(codex.credits,true,'a saved switch is shown');
const editor=apiUi.providerEditor(codex,null,new Node('article'));
const box=editor.querySelectorAll('input').find(node=>node.dataset.field==='providers.credits');
assert(box&&box.checked,'the codex editor offers the switch, on as saved');
assert(text(editor).includes('automatic reload'),'it says what automatic reload does');
change(box,false);
assert.equal(apiUi.providerBody(codex).credits,false,'a save sends the switch');
const claude=apiUi.providerEdit('y',{kind:'cli',cli:'claude',limits:{}});
assert(!apiUi.providerEditor(claude,null,new Node('article')).querySelectorAll('input').some(node=>node.dataset.field==='providers.credits'),'claude has no switch');
assert.equal(apiUi.providerBody(claude).credits,undefined,'claude sends none');
ctx.ui.setLang('ja');
assert(text(apiUi.providerEditor(codex,null,new Node('article'))).includes('クレジット'),'Japanese switch');
ctx.ui.setLang('en');
console.log('PASS: a codex entry may draw on credits');
}
// W4: the embedder's method is reviewed and agreed to before it is taken, a download's run is
// shown, Workers AI's values are checked before a save, and the token goes only into its request.
{
vm.runInContext('globalThis.w4={formOf,saveBody,embeddingDraft:()=>embeddingDraft,TEXT,refreshEmbedding,reloadEmbedding};',ctx);
const {w4}=ctx;
const json=(value,status=200)=>({ok:status>=200&&status<300,status,headers:{get:()=>'application/json'},json:async()=>value});
const embedding=(extra={})=>({provider:'none',
  workers_ai:{account_id:null,key_file:'/home/u/CF_WORKERS_AI_KEY.md',key:'missing',daily_requests:200,monthly_usd:1,
    usd_this_month:0,requests_today:0},
  local:{unavailable:null,files:'not_downloaded',dir:'/home/u/.oboete/models/bge-m3',held:0},...extra});
const settings=(extra={})=>({version:'v1',first_run:false,resident_supported:true,worker:{resident:true},view:{port:17373},
  summary:{curate:false,language:'Japanese',window_tokens:6000,idle_minutes:10},paid_usd_per_month:0,gemini:null,usd_this_month:0,
  stopped:null,inject:{session_start:true,session_start_note:true,session_start_list:true,per_prompt:false,correction:true,session_start_chars:6000,
    per_prompt_chars:1500,per_prompt_wait_ms:1500,correction_chars:800},capture:{store_prompts:false,tool_output:'full'},backup:{dir:null},
  redaction:{extra_rules:[],allowlist:[]},chain:[],providers:[],warnings:[],key_input:true,
  ranges:{session_start_chars:[0,20000],per_prompt_chars:[0,5000],per_prompt_wait_ms:[100,3000],correction_chars:[0,2000],window_tokens:[1000,100000],
    idle_minutes:[1,120],paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535],
    daily_requests:[1,100000],retention_days:[1,36500],my_work_calls:[1,10000]},
  my_work:{curate:false,calls_a_day:100,estimate:null},
  view_runtime:{port:17373,mode:'foreground'},embedding:embedding(),...extra});
validSettings=settings;
const posted=[];let answers=[];let settingsGets=0;let settingsAnswer=null;let runStatus=null;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){posted.push({url,body:JSON.parse(options.body)});return Promise.resolve(answers.shift());}
  if(url.startsWith('/api/settings?')){settingsGets++;return settingsAnswer??Promise.resolve(json(settings()));}
  if(url.startsWith('/api/embedding?')&&runStatus)return Promise.resolve(json(runStatus));
  return new Promise(()=>{});
};
const settle=()=>new Promise(resolve=>setImmediate(resolve));
const panel=document.getElementById('panel');
// One choice of method, the Search group's (decision 69, 69b-2); its More settings hold the local
// files' line and Workers AI's values.
const section=()=>panel.querySelector('.group-s-search');
const radio=value=>section().querySelectorAll('input').find(node=>node.dataset.simpleFocus===`embedding:${value}`);
const choose=value=>{const node=radio(value);node.checked=true;for(const cb of node.listeners.change)cb({target:node});};
const button=action=>section().querySelectorAll('button').find(node=>node.dataset.action===action);
const field=name=>panel.querySelectorAll('input').find(node=>node.dataset.field===name);
const type=(node,value)=>{node.value=value;for(const cb of node.listeners.input||[])cb({target:node});};
const plain=value=>JSON.parse(JSON.stringify(value));
ctx.ui.setLang('en');ctx.ui.setView('settings');

// Every string of the section in both languages.
for(const [key,[en,ja]] of Object.entries(w4.TEXT).filter(([key])=>key.startsWith('embedding_')||key.startsWith('group_search'))){
  assert(en&&ja&&en!==ja,`${key} has its English and Japanese`);
}
let f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(section(),'Settings shows the embedder');
assert.deepEqual(plain(section().querySelectorAll('input').filter(node=>node.dataset.field==='embedding.choice').map(node=>node.value)),['none','local','workers-ai']);
assert.equal(radio('none').checked,true);
assert(text(section().querySelector('.embedding-settings')).includes('not been downloaded'),'the local files state is shown in More settings');
assert(!button('embedding.start'),'no taking before the review');
const consent={choice:'local',dir:'/home/u/.oboete/models/bge-m3',check:false,
  get:[{host:'huggingface.co',bytes:2283937811,source:'model'},{host:'github.com',bytes:9125960,source:'runtime'}]};
const key='ab'.repeat(32);
answers=[json({preview_key:key,consent})];
choose('local');await settle();
assert.deepEqual(posted.at(-1),{url:'/api/embedding/preview',body:{choice:'local'}},'choosing a method reviews it');
let shown=text(section());
assert(shown.includes('huggingface.co')&&shown.includes('BAAI’s bge-m3, MIT License')&&shown.includes('github.com'),'what is downloaded, from where');
assert(shown.includes('2.3 GB')&&shown.includes('9 MB'),'sizes as the page says them elsewhere');
assert(shown.includes('no text or search leaves this computer'),'what leaves afterwards');
assert(button('embedding.start').disabled,'the method waits for the agreement');
const agree=section().querySelectorAll('input').find(node=>node.dataset.field==='embedding.confirmed');
agree.checked=true;for(const cb of agree.listeners.change)cb({target:agree});
assert(!button('embedding.start').disabled);
let finish;answers=[new Promise(resolve=>{finish=resolve;})];
button('embedding.start').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/embedding',body:{choice:'local',preview_key:key,confirmed:true}});
assert(text(section()).includes('Downloading the model'),'the run shows while its answer waits');
assert(radio('workers-ai').disabled,'no other method meanwhile');
const stopped={active:null,held:0,last:{choice:'local',phase:'failed',code:'embedding_no_space',held:0,get:2293063771}};
settingsAnswer=Promise.resolve(json({},503));
finish(json(stopped));
await settle();await settle();await settle();
assert(text(section()).includes('not enough free disk space'),'a stopped run says why');
assert(!radio('workers-ai').disabled);
// The settings could not be read after it: the run stays watched, and the next poll reads them.
assert(w4.embeddingDraft().watch,'a failed read after the answer keeps the run watched');
settingsAnswer=null;runStatus=stopped;
const before=settingsGets;
await w4.refreshEmbedding();await settle();
assert.equal(settingsGets,before+1,'the next poll reads the settings again');
assert(!w4.embeddingDraft().watch);
runStatus=null;

// A refusal is worded, and points at the field it needs.
answers=[json({code:'no_account',field:'embedding.choice'},422)];
choose('workers-ai');await settle();
assert(text(section()).includes('Cloudflare account ID first'));

// Workers AI's values: checked before the save, the account lowercased, an empty one kept.
f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
type(field('embedding.account_id'),'0123456789ABCDEF0123456789ABCDEF');
type(field('embedding.monthly_usd'),'2.5');
assert.deepEqual(plain(w4.saveBody().body.embedding),{account_id:'0123456789abcdef0123456789abcdef',daily_requests:200,monthly_usd:2.5});
type(field('embedding.account_id'),'');
assert.deepEqual(plain(w4.saveBody().body.embedding),{daily_requests:200,monthly_usd:2.5});
type(field('embedding.account_id'),'0123');
assert.equal(w4.saveBody().field,'embedding.account_id');
type(field('embedding.account_id'),'');type(field('embedding.monthly_usd'),'0');
assert.equal(w4.saveBody().field,'embedding.monthly_usd');
type(field('embedding.monthly_usd'),'1');type(field('embedding.daily_requests'),'100001');
assert.equal(w4.saveBody().field,'embedding.daily_requests');
assert.equal(w4.formOf(settings({embedding:undefined})).embedding,null,'an older viewer shows no section');
// The simple view appends no absent part: the page showed "null" under its lead wherever a summary
// service was ready, and where an older viewer sends no search section.
{
  const ready={chain:[{name:'groq',key:'ok',on:true,effective_model:'openai/gpt-oss-20b',model_rule:'any',differs:[]}],
    providers:[{name:'groq',selector:{source:'builtin',index:0},saved:{enabled:true,kind:'openai',base_url:'https://api.groq.com/openai/v1',
      model:'openai/gpt-oss-20b',key:'ok',timeout_s:60,limits:{}},effective:{on:true,order:0,model:'openai/gpt-oss-20b',timeout_s:60}}],stopped:[]};
  for(const [label,s] of [['ready',settings(ready)],['no search section',settings({embedding:undefined})]]){
    f=w4.formOf(s);ctx.ui.setForm(f);ctx.ui.drawSettings();
    const parts=panel.querySelector('.settings').children;
    assert(parts.length>3&&parts.every(part=>part!==null&&part!==undefined),`simple view, ${label}: no absent part`);
  }
  // Parity row 3: what was removed from the page is listed once, under Maintenance (decision 69,
  // 69b-2).
  const removed={items:[{kind:'card',id:'12.0',title:'Azimuth parser',repo_name:'fern',ts:30}]};
  f=w4.formOf({...settings(ready),hidden:removed});ctx.ui.setForm(f);ctx.ui.drawSettings();
  assert.equal(panel.querySelectorAll('.hidden-settings').length,1,'one list of what was removed');
  assert(text(panel.querySelector('.group-maintenance').querySelector('.hidden-settings')).includes('Azimuth parser'),
    'it is under maintenance');
  assert.deepEqual(w4.formOf(settings()).hidden,removed,
    "a save's answer, which has no list, keeps the one read (CodeRabbit on #658)");
  assert.equal(w4.formOf({...settings(),hidden:null}).hidden,null,'a list Settings could not read says so');
  ctx.ui.setForm(null);
}
f=w4.formOf(settings({embedding:embedding({workers_ai:{...embedding().workers_ai,account_id:'ACCT',daily_requests:500000,monthly_usd:0}})}));
ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual(plain(w4.saveBody().body.embedding),{daily_requests:500000,monthly_usd:0},'values config.toml has pass as they are');

// The token: typed into its field, sent once, never kept in the page's state.
f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
const token=field('embedding.key');
type(token,'SyntheticWorkersToken123');
answers=[json({...settings({embedding:embedding({workers_ai:{...embedding().workers_ai,key:'ok'}})}),key_saved:{durable:true}})];
const save=panel.querySelectorAll('button').find(node=>node.parentElement===token.parentElement);
save.click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/embedding/key',body:{key:'SyntheticWorkersToken123',version:'v1'}});
assert(!JSON.stringify(ctx.ui.getForm()).includes('SyntheticWorkersToken123'),'the token is not kept');
assert(text(section()).includes('Key found'));
f=w4.formOf(settings({key_input:false}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(!field('embedding.key')&&text(section()).includes('Linux and WSL'),'no form where a key cannot be registered');

// A start whose answer was lost: the poll finds the run over, and the saved method is read again.
f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
w4.embeddingDraft().watch=true;
runStatus={active:null,held:0,last:{choice:'workers-ai',phase:'done',code:null,held:0,get:0}};
let gets=settingsGets;
await w4.refreshEmbedding();await settle();
assert.equal(settingsGets,gets+1,'a watched run that ended reads the settings again');
assert(!w4.embeddingDraft().watch);
// Read again after a download, More settings' files line says the files are ready (decision 69,
// 69b-2: it is drawn with the page, outside the choice the run redraws).
f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
settingsAnswer=Promise.resolve(json(settings({embedding:embedding({provider:'local',local:{...embedding().local,files:'ready',held:2283937811}})})));
w4.embeddingDraft().watch=true;runStatus={active:null,held:0,last:{choice:'local',phase:'done',code:null,held:0,get:2283937811}};
await w4.refreshEmbedding();await settle();await settle();
assert(text(section().querySelector('.embedding-settings')).includes('files are ready'),'the files line is drawn again with the settings read');
settingsAnswer=null;runStatus={active:null,held:0,last:{choice:'workers-ai',phase:'done',code:null,held:0,get:0}};
// That read fails: the run stays watched, and the next poll reads the settings again.
w4.embeddingDraft().watch=true;settingsAnswer=Promise.resolve(json({},503));
await w4.refreshEmbedding();await settle();
assert(w4.embeddingDraft().watch,'a failed read keeps the ended run watched');
settingsAnswer=null;gets=settingsGets;
await w4.refreshEmbedding();await settle();
assert.equal(settingsGets,gets+1,'the next poll reads them');
assert(!w4.embeddingDraft().watch);
// A choice started while that read waits, whose answer is lost, keeps its watch after the read.
w4.embeddingDraft().watch=true;
let release;settingsAnswer=new Promise(resolve=>{release=resolve;});
const refreshing=w4.refreshEmbedding();await settle();
w4.embeddingDraft().watch=true;
release(json(settings()));await refreshing;await settle();
assert(w4.embeddingDraft().watch,'a newer choice keeps its watch');
settingsAnswer=null;
await w4.refreshEmbedding();await settle();
assert(!w4.embeddingDraft().watch);
// Another save's answer replaces the form while the reload reads (a save made before the
// download wrote the method): the reload reads again, and the method written last is shown.
let late;settingsAnswer=new Promise(resolve=>{late=resolve;});
gets=settingsGets;
const reloading=w4.reloadEmbedding();
ctx.ui.setForm(w4.formOf(settings({version:'v2'})));
const local=json(settings({version:'v3',embedding:embedding({provider:'local'})}));
settingsAnswer=Promise.resolve(local);late(local);await reloading;await settle();
assert.equal(ctx.ui.getForm().embedding.provider,'local','a save answered during the reload does not keep an older method');
assert.equal(settingsGets,gets+2,'the reload read again for the form then shown');
settingsAnswer=null;runStatus=null;
f=w4.formOf(settings({embedding:embedding({provider:'workers-ai'})}));ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(section()).includes('sent to the account saved here'),'the destination beside the account');

ctx.ui.setLang('ja');f=w4.formOf(settings());ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(text(panel).includes('意味で記憶を探す')&&text(section()).includes('このパソコンで探す')
  &&text(section()).includes('モデルはまだダウンロードしていません'),'Japanese section');
ctx.ui.setLang('en');
console.log('PASS: the embedder is reviewed, agreed and run; its values and token');

// docs/my-work-model.md J1: the judge. With no key, no switch and nothing in the save; the key
// typed, sent once and kept nowhere, turns it on; its switch then saves; a test says its outcome.
{
  for(const [key,[en,ja]] of Object.entries(w4.TEXT).filter(([key])=>key.startsWith('judge_'))){
    assert(en&&ja&&en!==ja,`${key} has its English and Japanese`);
  }
  const dests=(keys={})=>[['jev',0.042,false],['clef',0.24,false],['clef-flash',0.038,false],['d1',0.02,false],['d1-local',0,true]]
    .map(([id,usd,local])=>({id,key:local?'none':(keys[id]??'missing'),usd_per_mtok_in:usd,local}));
  const judge=(extra={})=>({set:false,enabled:false,destination:'jev',destinations:dests(),key:'missing',key_file:null,
    cloudflare_account_id:null,daily_budget:null,calls_today:0,usd_this_month:0,...extra});
  const on=judge({set:true,enabled:true,key:'ok',destinations:dests({jev:'ok'}),key_file:'/home/u/.local/share/oboete/keys/k1',daily_budget:300,calls_today:2,usd_this_month:0.0004});
  const judged=()=>panel.querySelector('.judge-settings');
  const switchOf=()=>judged().querySelectorAll('input').find(node=>node.type==='checkbox');
  assert.equal(w4.formOf(settings()).judge,null,'an older server shows no section');
  f=w4.formOf(settings({judge:judge()}));ctx.ui.setForm(f);ctx.ui.drawSettings();
  assert(judged()&&text(judged()).includes('no key yet'),'the judge with no key');
  assert(!switchOf(),'no switch before a key');
  assert(!('judge' in w4.saveBody().body),'and nothing of it in a save');
  const k=field('judge.key');
  type(k,'apikey_0123456789abcdef_Synthetic');
  answers=[json({...settings({judge:on}),key_saved:{durable:true}})];
  judged().querySelectorAll('button').find(node=>node.parentElement===k.parentElement).click();await settle();
  assert.deepEqual(posted.at(-1),{url:'/api/judge/key',body:{key:'apikey_0123456789abcdef_Synthetic',version:'v1',destination:'jev'}});
  assert(!JSON.stringify(ctx.ui.getForm()).includes('apikey_0123456789abcdef_Synthetic'),'the key is not kept');
  assert(switchOf().checked,'a key saved turns it on');
  assert(text(judged()).includes('Key found')&&text(judged()).includes('2 of 300 calls'),'its key and calls');
  change(switchOf(),false);
  assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:false},'the switch saves');
  // Decision 64: its calls a day, sent only when changed, or No cap's 4294967295.
  ctx.ui.getForm().ranges.no_cap=4294967295;
  assert.equal(field('judge.daily_budget').value,'300');
  type(field('judge.daily_budget'),'50');
  assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:false,daily_budget:50},'a typed calls a day is sent');
  for(const bad of ['0','1001','']){type(field('judge.daily_budget'),bad);assert.equal(w4.saveBody().field,'judge.daily_budget',`calls ${bad} refused before sending`);}
  change(field('judge.no_cap'),true);
  assert.equal(field('judge.daily_budget').disabled,true);
  assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:false,daily_budget:4294967295},'no cap is sent');
  change(field('judge.no_cap'),false);type(field('judge.daily_budget'),'300');
  assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:false},'left as saved, it is not sent');
  {
    const ranges={...settings().ranges,no_cap:4294967295};
    ctx.ui.setForm(w4.formOf(settings({ranges,judge:{...on,daily_budget:4294967295}})));ctx.ui.drawSettings();
    assert.deepEqual([field('judge.no_cap').checked,field('judge.daily_budget').value,field('judge.daily_budget').disabled],[true,'',true]);
    assert(text(judged()).includes('2 calls, with no cap')&&!text(judged()).includes('4294967295'),'its usage with no cap');
    f=w4.formOf(settings({judge:on}));ctx.ui.setForm(f);ctx.ui.drawSettings();change(switchOf(),false);
  }
  // After a test the judge's numbers are read again, and the switch as edited stays.
  settingsAnswer=Promise.resolve(json(settings({judge:{...on,calls_today:3}})));
  for(const [outcome,words] of [['ok','Jev (Typesafe) answered: the connection works'],['error','Check the key'],['budget','used up'],['other','could not run']]){
    answers=[json({outcome})];
    judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();await settle();
    assert.deepEqual(posted.at(-1),{url:'/api/judge/test',body:{confirmed:true}});
    assert(document.getElementById('status').textContent.includes(words),`the test's ${outcome}`);
    assert(text(judged()).includes('3 of 300 calls'),`its calls read again after ${outcome}`);
    assert.equal(switchOf().checked,false,'the switch as edited stays');
  }
  settingsAnswer=null;
  // A key saved while a test's numbers are read again has the page: the test's outcome is not
  // said, and the read is not merged.
  {
    let release;settingsAnswer=new Promise(resolve=>{release=resolve;});
    answers=[json({outcome:'ok'})];
    const testButton=judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test');
    document.getElementById('status').textContent='';
    testButton.click();await settle();
    const replaced=w4.formOf(settings({judge:{...on,calls_today:7}}));ctx.ui.setForm(replaced);ctx.ui.drawSettings();
    release(json(settings({judge:{...on,calls_today:8}})));await settle();await settle();
    assert.equal(ctx.ui.getForm(),replaced,'the newer form stays');
    assert(!document.getElementById('status').textContent.includes('the connection works'),'an older test is not said');
    settingsAnswer=null;
  }
  // A save running when the read comes back keeps its place: the read is not merged.
  {
    let release;settingsAnswer=new Promise(resolve=>{release=resolve;});
    answers=[json({outcome:'ok'})];
    const mine=ctx.ui.getForm();
    judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();
    panel.querySelector('.settings').inert=true;
    release(json(settings({judge:{...on,calls_today:9}})));await settle();await settle();
    assert.equal(ctx.ui.getForm(),mine,'a running save keeps its place');
    assert(document.getElementById('status').textContent.includes('the connection works'),'the outcome is still said');
    panel.querySelector('.settings').inert=false;settingsAnswer=null;
  }
  // #766: settings left for another tab and opened again, as the tabs do it (`show`), while a test
  // is out: the old form stays until the new visit is read. The old visit's test says nothing and
  // merges nothing, whether its answer or its read comes back after; the new visit's read draws.
  for(const out of ['answer','read']){
    const fetchBefore=ctx.fetch;
    const reads=[];let answered;
    ctx.fetch=(url,options)=>{
      if(options?.method==='POST'){
        posted.push({url,body:JSON.parse(options.body)});
        return out==='answer'?new Promise(resolve=>{answered=resolve;}):Promise.resolve(json({outcome:'ok'}));
      }
      if(url.startsWith('/api/settings?'))return new Promise(resolve=>{reads.push(resolve);});
      if(url.startsWith('/api/privacy?'))return Promise.resolve(json({available:false,repositories:[],rescan:{state:'unavailable'}}));
      if(url.startsWith('/api/hidden?'))return Promise.resolve(json({code:'hide_unavailable'},503));
      return new Promise(()=>{});
    };
    const mine=ctx.ui.getForm();
    judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();
    ctx.ui.setView('my_work');void ctx.ui.show();ctx.ui.setView('settings');void ctx.ui.show();await settle();
    // The new visit's read is the last one asked; the test's own, when it was out, the first.
    const visit=reads.at(-1);
    if(out==='answer')answered(json({outcome:'ok'}));
    else reads[0](json(settings({judge:{...on,calls_today:11}})));
    await settle();await settle();
    if(out==='answer')assert.equal(reads.length,1,'an earlier visit\'s test reads nothing more');
    // A read the old test would ask after its answer comes back too.
    for(const r of reads.slice(reads.indexOf(visit)+1))r(json(settings({judge:{...on,calls_today:11}})));
    await settle();await settle();await settle();
    assert.equal(ctx.ui.getForm(),mine,`an earlier visit's test merges nothing (${out})`);
    assert(!document.getElementById('status').textContent.includes('the connection works'),`and says nothing (${out})`);
    assert(!text(judged()).includes('11 of 300 calls'),`its read is not drawn (${out})`);
    visit(json(settings({judge:{...on,calls_today:12}})));await settle();await settle();await settle();
    assert.notEqual(ctx.ui.getForm(),mine,`the new visit has its own form (${out})`);
    assert(text(judged()).includes('12 of 300 calls'),`the new visit's read is drawn (${out})`);
    assert(!document.getElementById('status').textContent.includes('the connection works'),`the old test said nothing after (${out})`);
    ctx.fetch=fetchBefore;
  }
  answers=[json({code:'missing_key',field:'judge.test'},422)];
  judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();
  assert(document.getElementById('status').textContent.includes('Paste the key first'),'a refusal in words');
  ctx.ui.setLang('ja');f=w4.formOf(settings({judge:on}));ctx.ui.setForm(f);ctx.ui.drawSettings();
  assert(text(judged()).includes('判定役')&&text(judged()).includes('直近 24 時間: 300 回のうち 2 回'),'Japanese section');
  ctx.ui.setLang('en');
  console.log('PASS: the judge: no key, the key once, its switch, its test, JA/EN');
  // docs/my-work-model.md J7: where the judge sends. The list by name, the chosen one's price under it; a destination
  // chosen shows its note and its key at once and is saved with the page; Cloudflare's token goes
  // with the account ID; d1 on this PC takes no key and turns on with none.
  {
    const pick=()=>judged().querySelectorAll('select').find(node=>node.dataset.field==='judge.destination');
    const choose=id=>{const s=pick();s.value=id;for(const cb of s.listeners.change||[])cb({target:s});};
    f=w4.formOf(settings({judge:on}));ctx.ui.setForm(f);ctx.ui.drawSettings();
    assert.deepEqual(pick().options.map(o=>o.value),['jev','clef','clef-flash','d1','d1-local'],'the five of the list');
    assert.equal(pick().value,'jev');
    // The list names them only, so a name is not cut short; the chosen one's price is under it.
    assert(text(pick()).includes('Clef-flash (Cloudflare Workers AI)')&&!text(pick()).includes('USD'),'each by its name');
    const priced=()=>pick().parentElement.querySelectorAll('.note').map(text);
    assert(priced().includes('USD 0.042 for a million input tokens'),'the chosen one\'s price');
    // Each one's own price as it is chosen; the price describes the list, and the list drawn again
    // keeps the focus.
    for(const [id,price] of [['clef','USD 0.24 for a million input tokens'],['d1','USD 0.02 for a million input tokens'],
      ['clef-flash','USD 0.038 for a million input tokens'],['d1-local','Free, and nothing leaves this PC'],['jev','USD 0.042 for a million input tokens']]){
      choose(id);
      assert(priced().includes(price),`${id}'s price`);
      const describedBy=pick().attrs['aria-describedby'];
      assert(describedBy&&pick().parentElement.querySelectorAll('.note').some(n=>n.id===describedBy&&text(n)===price),`${id}'s price describes the list`);
      assert.equal(document.activeElement,pick(),`the list keeps the focus after ${id} is chosen`);
    }
    // Each destination's note, as its TEXT says where it is sent.
    const noted=id=>text(judged()).includes(w4.TEXT[`judge_note_${id}`][0]);
    assert(noted('jev')&&!field('judge.account_id'),'where Jev is sent, with no account');
    choose('clef-flash');
    assert(noted('clef_flash'),'where Clef-flash is sent');
    assert(priced().includes('USD 0.038 for a million input tokens'),'and its price');
    assert(text(judged()).includes('Saved: Jev (Typesafe)'),'the saved one noted');
    assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:true,destination:'clef-flash'},'the choice saves with the page');
    type(field('judge.account_id'),' 0123456789ABCDEF0123456789ABCDEF ');
    // A test's numbers read again keep the destination chosen and the account typed.
    settingsAnswer=Promise.resolve(json(settings({judge:on})));
    answers=[json({outcome:'ok'})];
    judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();await settle();
    settingsAnswer=null;
    assert.equal(pick().value,'clef-flash','the choice kept through a read');
    assert.equal(field('judge.account_id').value,' 0123456789ABCDEF0123456789ABCDEF ','the account typed kept through a read');
    const k=field('judge.key');
    type(k,'cfut_0123456789abcdef_Synthetic');
    const chosen={...on,destination:'clef-flash',destinations:dests({jev:'ok',clef:'ok','clef-flash':'ok'}),
      cloudflare_account_id:'0123456789abcdef0123456789abcdef'};
    answers=[json({...settings({judge:chosen}),key_saved:{durable:true}})];
    judged().querySelectorAll('button').find(node=>node.parentElement===k.parentElement).click();await settle();
    assert.deepEqual(posted.at(-1),{url:'/api/judge/key',body:{key:'cfut_0123456789abcdef_Synthetic',version:'v1',
      destination:'clef-flash',account_id:'0123456789abcdef0123456789abcdef'}},'the token with its destination and account');
    assert.equal(pick().value,'clef-flash','the key saved chose it');
    assert.equal(field('judge.account_id').value,'0123456789abcdef0123456789abcdef','the account as saved');
    assert(!('destination' in w4.saveBody().body.judge),'saved, it is not sent again');
    // d1 on this PC: no key; with no `[judge]` yet it turns on with none.
    f=w4.formOf(settings({judge:judge()}));ctx.ui.setForm(f);ctx.ui.drawSettings();
    assert(!switchOf(),'no switch before a key');
    assert(!text(judged()).includes('Saved:'),'nothing saved is said with no [judge]');
    choose('d1-local');
    assert(noted('d1_local')&&!field('judge.key'),'d1 on this PC: its address, and no key');
    assert(priced().includes('Free, and nothing leaves this PC'),'and that it stays here');
    change(switchOf(),true);
    assert.deepEqual(plain(w4.saveBody().body.judge),{enabled:true,destination:'d1-local'},'turned on with no key');
    choose('d1');
    assert(noted('d1')&&!switchOf(),'Liquid\'s, its key not there yet');
    assert(!('judge' in w4.saveBody().body),'one with no key is not saved');
    ctx.ui.setLang('ja');ctx.ui.drawSettings();
    assert(text(judged()).includes('送り先')&&text(pick()).includes('この PC の d1(Ollama)'),'Japanese list');
    choose('d1-local');
    assert(priced().includes('無料。この PC の外に出ません'),'and its price in Japanese');
    choose('clef');
    assert(priced().includes('入力 100 万トークンで 0.24 ドル'),'a price in dollars in Japanese');
    ctx.ui.setLang('en');
    // #786: a failed test of d1 on this PC says what to check there, and what is asked of the
    // program at its address.
    const local={...on,destination:'d1-local',key:'none',key_file:null};
    f=w4.formOf(settings({judge:local}));ctx.ui.setForm(f);ctx.ui.drawSettings();
    const testSays=async (outcome,reason)=>{
      settingsAnswer=Promise.resolve(json(settings({judge:local})));
      answers=[json({outcome,...(reason?{reason}:{})})];
      judged().querySelectorAll('button').find(node=>node.dataset.field==='judge.test').click();await settle();await settle();
      settingsAnswer=null;
      return document.getElementById('status').textContent;
    };
    const unanswered=await testSays('error');
    assert(unanswered.includes('ollama pull d1')&&!unanswered.includes('Check the key')&&!unanswered.includes('Nothing was sent'),'what to check for d1 on this PC');
    // #791: a request the check refused went nowhere, and the page says why.
    const none=await testSays('refused','no listener');
    assert(none.includes('Nothing was sent')&&none.includes('nothing waits')&&none.includes('Start Ollama'),'nothing listens');
    const untrusted=await testSays('refused','untrusted listener');
    // Words only this reason's text has (#794): mapped back to the general refusal, the test fails.
    assert(untrusted.includes('Nothing was sent')&&untrusted.includes('Stop that program')&&!untrusted.includes('Check that Ollama is running'),
      'another user\'s program');
    const unread=await testSays('refused','listeners unread');
    assert(unread.includes('Nothing was sent')&&unread.includes('could not read')&&!unread.includes('Ollama\'s own account'),'the tables unread');
    const older=await testSays('refused');
    assert(older.includes('Nothing was sent'),'a refusal with no reason given');
    for(const key of ['missing_key','bad_account'])assert(w4.TEXT[key][0]&&w4.TEXT[key][1]&&w4.TEXT[key][0]!==w4.TEXT[key][1],`${key} in words`);
    console.log('PASS: J7: the list and its prices, a destination\'s key, d1 on this PC with none');
  }
}
}

// Simple controls save only their own change, using the existing whole-settings operation.
{
vm.runInContext("globalThis.simpleUi={formOf,TEXT,embeddingDraft:()=>embeddingDraft,renderEmbedding};",ctx);
const plain=value=>JSON.parse(JSON.stringify(value));
const privacy={available:true,repositories:[],folders:[],rescan:{state:'idle',processed:null,total:null}};
let saved={version:'simple-v1',first_run:true,resident_supported:true,worker:{resident:false},view:{port:17373},
  summary:{curate:false,language:'Japanese',window_tokens:6000,idle_minutes:10},paid_usd_per_month:2,gemini:null,usd_this_month:0,
  stopped:[],inject:{session_start:true,session_start_note:true,session_start_list:true,per_prompt:false,correction:true,
    session_start_chars:6000,per_prompt_chars:1500,per_prompt_wait_ms:1500,correction_chars:800},capture:{store_prompts:false,tool_output:'full'},
  redaction:{enabled:true,extra_rules:[],allowlist:[]},privacy,backup:{dir:null},warnings:[],key_input:true,
  embedding:{provider:'none',workers_ai:{account_id:null,key_file:'test-key.md',key:'missing',daily_requests:200,monthly_usd:1,
    usd_this_month:0,requests_today:0},local:{unavailable:null,files:'not_downloaded',dir:'/tmp/oboete-test/models',held:0}},
  ranges:{paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535],daily_requests:[1,100000],
    session_start_chars:[0,20000],per_prompt_chars:[0,5000],per_prompt_wait_ms:[100,3000],correction_chars:[0,2000],window_tokens:[0,100000],idle_minutes:[0,120],retention_days:[1,36500],my_work_calls:[1,10000]},
  my_work:{curate:false,calls_a_day:100,estimate:null},
  chain:[{name:'groq',on:true,key:'missing',effective_model:'openai/gpt-oss-120b',model_rule:'any',model:null,
    daily_budget:null,timeout_s:null,differs:[]}],
  providers:[{name:'groq',selector:{source:'builtin',index:0},saved:{enabled:true,kind:'openai',base_url:'https://api.groq.com/openai/v1',
    model:'openai/gpt-oss-120b',key:'missing',limits:{}},effective:{on:true}}]};
const requests=[];let refuseNext=false;
ctx.fetch=(url,options)=>{
  if(!options?.method){assert(url.startsWith('/api/privacy?'));return Promise.resolve(diagnosticResponse(privacy));}
  const body=JSON.parse(options.body);requests.push({url,body});
  assert.equal(url,'/api/settings','ordinary simple changes reuse the existing Settings write');
  if(refuseNext){refuseNext=false;return Promise.resolve(diagnosticResponse({code:'write_failed'},500));}
  const prior=saved.chain;
  saved={...saved,...body,version:'simple-v'+(requests.length+1)};
  // The response includes the same effective facts as GET /api/settings.
  saved.chain=saved.chain.map(row=>({...prior.find(entry=>entry.name===row.name),...row,
    effective_model:row.model||prior.find(entry=>entry.name===row.name).effective_model}));
  return Promise.resolve(diagnosticResponse(plain(saved)));
};
let f=ctx.simpleUi.formOf(plain(saved));
f.summary.language='Unsaved detailed language';f.inject.session_start_chars='7777';f.paid_usd_per_month='99';
f.my_work.curate=true;f.my_work.calls_a_day='250';f.paidNoCap=true;f.my_work.noCap=true;
f.retention.forever=false;f.retention.edit='90';
ctx.ui.setForm(f);ctx.ui.setView('settings');ctx.ui.setLang('ja');ctx.ui.drawSettings();
assert.equal(panel.querySelectorAll('.simple-block').length,8,'all eight blocks are present');
assert(text(panel).includes('要約に使える AI がまだない'),'an unusable home has a clear first action');
assert(panel.querySelectorAll('.simple-block').every(node=>!node.closest('form')),'no simple control is inside a general form');
assert(!text(panel).includes('4294967295'),'field ranges stay in detailed settings');
assert(panel.querySelector('.simple-provider').querySelectorAll('input').some(node=>node.dataset.field==='providers.key'),
  'the existing write-only key field is inline');
const field=key=>panel.querySelectorAll('input').find(node=>node.dataset.simpleFocus===key);
const type=(node,value)=>{node.value=value;for(const cb of node.listeners.input||[])cb({target:node});};
const settle=()=>new Promise(resolve=>setImmediate(resolve));
change(field('summary.curate'),true);await settle();
assert.equal(requests.length,1);
assert.equal(requests[0].body.summary.curate,true);
assert.equal(requests[0].body.summary.language,'Japanese','autosave does not commit an unrelated detailed draft');
assert.equal(requests[0].body.inject.session_start_chars,6000);
assert.equal(requests[0].body.paid_usd_per_month,2);
assert.equal(requests[0].body.worker,undefined,'a first simple save does not activate suggested resident mode');
assert.equal(ctx.ui.getForm().summary.language,'Unsaved detailed language','unrelated drafts survive the saved response');
assert.equal(ctx.ui.getForm().inject.session_start_chars,'7777');
assert.deepEqual([ctx.ui.getForm().retention.forever,ctx.ui.getForm().retention.edit],[false,'90'],'an unsaved period survives too');
assert.deepEqual([ctx.ui.getForm().my_work.curate,ctx.ui.getForm().my_work.calls_a_day],[true,'250'],'an unsaved My Work draft survives too (Codex on #671)');
assert.deepEqual([ctx.ui.getForm().paidNoCap,ctx.ui.getForm().my_work.noCap],[true,true],'unsaved No cap switches survive too (decision 64)');
assert(text(panel).includes('保存しました'),'a saved receipt is beside the changed block');
assert(text(panel).includes('記録がたまると、ここに見積もりが出ます。'),'My Work\'s block before anything is recorded');
change(field('my_work.curate'),true);await settle();
assert.deepEqual(requests.at(-1).body.my_work,{curate:true,calls_a_day:100},'My Work\'s switch sends the saved bound');
assert.equal(requests.at(-1).body.summary.curate,true,'and keeps the saved summary switch');
assert.equal(field('my_work.curate').checked,true,'the saved switch shows');
type(field('paid_usd_per_month'),'3.5');for(const cb of field('paid_usd_per_month').listeners.blur)cb();await settle();
assert.equal(requests.at(-1).body.paid_usd_per_month,3.5);
assert.equal(requests.at(-1).body.summary.language,'Japanese');
type(field('paid_usd_per_month'),'4.25');ctx.ui.drawSettings();
assert.equal(field('paid_usd_per_month').value,'4.25','a typed limit survives a redraw before it is saved');
for(const cb of field('paid_usd_per_month').listeners.blur)cb();await settle();
assert.equal(requests.at(-1).body.paid_usd_per_month,4.25);
ctx.ui.drawSettings();
assert.equal(field('paid_usd_per_month').value,'4.25','the saved limit shows once its draft is saved');
change(field('inject.per_prompt'),true);await settle();
assert.equal(requests.at(-1).body.inject.per_prompt,true);
assert.equal(requests.at(-1).body.inject.session_start_chars,6000,'a switch keeps saved sizes');
const before=requests.length;
change(field('redaction.enabled'),false);
assert.equal(requests.length,before,'turning masking off first opens its warning');
let confirm=panel.querySelectorAll('button').find(node=>node.dataset.simpleFocus==='mask.confirm');
assert(confirm&&text(panel).includes(ctx.simpleUi.TEXT.redaction_off_warning[1]));
confirm.click();await settle();
assert.equal(requests.length,before+1);
assert.equal(requests.at(-1).body.redaction.enabled,false);
change(field('redaction.enabled'),true);await settle();
assert.equal(requests.at(-1).body.redaction.enabled,true);
// Decision 64: No cap for the monthly limit waits for its confirmation, then saves null; a limit
// saved off is set aside until No cap is turned off and a number is typed.
{
  const sent=requests.length;
  const button=key=>panel.querySelectorAll('button').find(node=>node.dataset.simpleFocus===key);
  change(field('paid_usd_per_month.no_cap'),true);
  assert.equal(requests.length,sent,'No cap first opens its warning');
  assert(field('paid_usd_per_month').disabled&&button('cap.confirm')&&text(panel).includes(ctx.simpleUi.TEXT.no_cap_paid_warning[1]));
  button('cap.cancel').click();
  assert.deepEqual([field('paid_usd_per_month.no_cap').checked,field('paid_usd_per_month').disabled,requests.length],[false,false,sent],
    'cancelled, nothing is saved');
  change(field('paid_usd_per_month.no_cap'),true);
  button('cap.confirm').click();await settle();
  assert.deepEqual([requests.length,requests.at(-1).body.paid_usd_per_month],[sent+1,null],'confirmed, the limit is saved off');
  assert.deepEqual([field('paid_usd_per_month.no_cap').checked,field('paid_usd_per_month').disabled,field('paid_usd_per_month').value],[true,true,'']);
  change(field('summary.curate'),false);await settle();
  assert.equal(requests.at(-1).body.paid_usd_per_month,null,'another simple save keeps it off');
  change(field('paid_usd_per_month.no_cap'),false);
  assert.equal(field('paid_usd_per_month').disabled,false,'turned off, the limit can be typed');
  change(field('paid_usd_per_month.no_cap'),true);
  assert.deepEqual([requests.length,field('paid_usd_per_month').disabled,Boolean(button('cap.confirm'))],[sent+2,true,false],
    'back to the no cap saved, nothing is confirmed or sent');
  change(field('paid_usd_per_month.no_cap'),false);
  type(field('paid_usd_per_month'),'6');for(const cb of field('paid_usd_per_month').listeners.blur)cb();await settle();
  assert.equal(requests.at(-1).body.paid_usd_per_month,6,'a typed limit is a cap again');
  assert.equal(field('paid_usd_per_month.no_cap').checked,false);
}
for(const language of ['ja','en']){
  ctx.ui.setLang(language);ctx.ui.drawSettings();
  for(const [key,pair] of Object.entries(ctx.simpleUi.TEXT).filter(([key])=>key.startsWith('simple_'))){
    assert.equal(pair.length,2,`${key} is bilingual`);assert(pair.every(value=>typeof value==='string'&&value.length));
  }
  assert.equal(panel.querySelectorAll('.simple-block').length,8);
}
assert.equal(requests.filter(request=>request.url!=='/api/settings').length,0,'rendering never calls a provider');
saved.chain.push({...saved.chain[0],name:'openrouter',effective_model:'nvidia/nemotron-3-super-120b-a12b:free',model_rule:'free'});
saved.providers.push({...saved.providers[0],name:'openrouter',selector:{source:'builtin',index:1},
  saved:{...saved.providers[0].saved,base_url:'https://openrouter.ai/api/v1'}});
f=ctx.simpleUi.formOf(plain(saved));f.chain.reverse();ctx.ui.setForm(f);ctx.ui.drawSettings();
assert.deepEqual(panel.querySelectorAll('.simple-provider').map(node=>node.dataset.providerName),['groq','openrouter'],
  'simple order follows saved settings even when a detailed order is pending');
panel.querySelectorAll('button').find(node=>node.dataset.simpleFocus==='move:groq:1').click();await settle();
assert.deepEqual(requests.at(-1).body.chain.map(row=>row.name),['openrouter','groq'],'the arrow changes the displayed saved order');
const model=panel.querySelectorAll('select').find(node=>node.dataset.simpleFocus==='model:groq');
model.value='openai/gpt-oss-20b';for(const cb of model.listeners.change)cb({target:model});await settle();
assert.equal(requests.at(-1).body.chain.find(row=>row.name==='groq').model,'openai/gpt-oss-20b','a model pick uses the name overlay');
saved.redaction.enabled=false;ctx.ui.setForm(ctx.simpleUi.formOf(plain(saved)));ctx.ui.drawSettings();
refuseNext=true;change(field('redaction.enabled'),true);await settle();
assert.equal(field('redaction.enabled').checked,false,'a failed save never claims that masking has been enabled');
assert(panel.querySelectorAll('.simple-saved').every(node=>!text(node).trim()),'a failed change clears the previous saved receipt');
const limitField=field('paid_usd_per_month');document.activeElement=limitField;
ctx.simpleUi.embeddingDraft().focus='embedding:local';ctx.simpleUi.renderEmbedding();
assert(document.activeElement===limitField,'embedding polling does not take focus from another setting');
document.activeElement=null;
ctx.simpleUi.embeddingDraft().focus=undefined;ctx.simpleUi.renderEmbedding();
assert(document.activeElement===null,'a passive embedding read never focuses an unrelated unmarked control');
// Entries that read one key file take the key once: the first of them shows the field, the
// others say whose key they use.
saved.redaction.enabled=true;
saved.chain=[{name:'groq',on:true,key:'missing',key_file:'/k/GROQ_API_KEY.md',effective_model:'openai/gpt-oss-120b',model_rule:'any',
  model:null,daily_budget:null,timeout_s:null,differs:[]},{name:'groq-20b',on:true,key:'missing',key_file:'/k/GROQ_API_KEY.md',
  effective_model:'openai/gpt-oss-20b',model_rule:'any',model:null,daily_budget:null,timeout_s:null,differs:[]}];
saved.providers=[saved.providers[0],{...saved.providers[0],name:'groq-20b',selector:{source:'builtin',index:1}}];
ctx.ui.setForm(ctx.simpleUi.formOf(plain(saved)));ctx.ui.setLang('ja');ctx.ui.drawSettings();
const rowOf=name=>panel.querySelectorAll('.simple-provider').find(node=>node.dataset.providerName===name);
const keyInput=row=>row.querySelectorAll('input').some(node=>node.dataset.field==='providers.key');
assert(keyInput(rowOf('groq')),'the first entry of a key file takes the key');
assert(!keyInput(rowOf('groq-20b')),'a later entry of the same key file has no second key field');
assert(text(rowOf('groq-20b')).includes('と同じキーを使います'),'a later entry says whose key it uses');
// A detailed order still unsaved does not move the key field: the rows follow the saved chain.
f=ctx.simpleUi.formOf(plain(saved));f.chain.reverse();ctx.ui.setForm(f);ctx.ui.drawSettings();
assert(keyInput(rowOf('groq')),'the first saved entry keeps the key field under a pending detailed order');
assert(!keyInput(rowOf('groq-20b')),'a pending detailed order gives the later entry no key field');
// Decision 69: the simple page's groups fold as claude-mem's settings do. What agents are given
// and the AI for summaries open first; each closed group says on its heading what it holds now.
// The AI in use are one line each, in the order they are tried; those turned off fold together.
{
  const row=(name,on)=>({name,on,key:'ok',effective_model:'model-'+name,model_rule:'any',model:null,daily_budget:null,timeout_s:null,differs:[]});
  const provider=(name,index)=>({...saved.providers[0],name,selector:{source:'builtin',index}});
  saved={...plain(saved),usd_this_month:0.385626,summary:{...saved.summary,curate:true},
    inject:{...saved.inject,session_start:true,per_prompt:false},redaction:{...saved.redaction,enabled:true},
    my_work:{...saved.my_work,curate:false},
    privacy:{available:true,repositories:[{label:'github.com/acme/old',selector:'r1',never_recorded:true},
      {label:'github.com/acme/live',selector:'r2',never_recorded:false}],folders:[{label:'scratch',selector:'f1'}],
      patterns:[{label:'tmp-*',selector:'p1'}],rescan:{state:'idle',processed:null,total:null}},
    chain:[row('groq',true),row('groq-20b',false),row('openrouter',true)],
    providers:[provider('groq',0),provider('groq-20b',1),provider('openrouter',2)]};
  ctx.ui.setForm(ctx.simpleUi.formOf(plain(saved)));ctx.ui.setLang('ja');ctx.ui.drawSettings();
  const all=()=>panel.querySelectorAll('.settings-group');
  const titleOf=g=>text(g.querySelector('.group-title')),stateOf=g=>text(g.querySelector('.group-desc'));
  // Decision 69 (69b-2): one page. The guide on top; This computer and Maintenance, which the mock
  // has not, at the bottom, closed.
  assert.deepEqual(all().map(titleOf),['はじめに','エージェントに渡す記憶','要約に使う AI','AI が書く My Work','意味で記憶を探す',
    '記録しないものと秘密','接続するエージェント','このパソコン','診断と保守'],'what is given first, then the AI, as the owner asked');
  assert.deepEqual(all().map(g=>g.open),[true,true,true,false,false,false,false,true,false],
    'two groups open, the rest closed; on a new home, as here, the guide and This computer too');
  const groups=()=>all().slice(1,7);
  assert(stateOf(groups()[0]).includes('セッション開始時：オン')&&stateOf(groups()[0]).includes('依頼ごと：オフ'),stateOf(groups()[0]));
  assert(stateOf(groups()[1]).includes('2 個')&&stateOf(groups()[1]).includes('$0.39'),stateOf(groups()[1]));
  assert(stateOf(groups()[2]).includes('オフ'),'My Work says it is off');
  assert(stateOf(groups()[4]).includes('3 か所')&&stateOf(groups()[4]).includes('オン'),stateOf(groups()[4]));
  assert(text(panel).includes('$0.39')&&!text(panel).includes('0.385626'),'the month\'s spend in cents');
  assert.deepEqual(panel.querySelectorAll('.simple-provider').map(node=>node.dataset.providerName),['groq','openrouter','groq-20b'],
    'the AI in use first, in their order; the one turned off after them');
  const unused=panel.querySelector('.simple-unused');
  assert(text(unused.querySelector('summary')).includes('使っていない AI (1)'),text(unused.querySelector('summary')));
  assert.deepEqual(unused.querySelectorAll('.simple-provider').map(node=>node.dataset.providerName),['groq-20b']);
  assert.equal(unused.open,false,'folded until opened');
  assert(!panel.querySelectorAll('button').some(node=>node.dataset.simpleFocus?.startsWith('move:groq-20b')),'an AI turned off has no place to move');
  assert.deepEqual(panel.querySelectorAll('.simple-order').map(text),['1','2'],'the AI in use are numbered in their order');
  // The catalog's "find it in the list" opens the folds around an AI turned off and focuses its
  // switch (Codex, cubic and CodeRabbit on #801).
  const listed=panel.querySelector('.simple-catalog').querySelectorAll('li').find(li=>text(li.querySelector('strong'))==='Groq・GPT OSS 20B');
  listed.querySelector('button').click();
  const offRow=panel.querySelector('.simple-unused').querySelectorAll('.simple-provider').find(node=>node.dataset.providerName==='groq-20b');
  assert.equal(panel.querySelector('.simple-unused').open,true,'the AI not in use open');
  assert.equal(offRow.querySelector('details').open,true,'and the AI\'s own line');
  assert.equal(document.activeElement?.dataset.simpleFocus,'on:groq-20b','its switch has the focus');
  panel.querySelector('.simple-unused').open=false;offRow.querySelector('details').open=false;
  const places=panel.querySelector('.simple-places').parentElement;
  assert.equal(places.tagName,'DETAILS');assert(text(places.querySelector('summary')).includes('3 か所'));assert.equal(places.open,false);
  // Down from the first skips the AI turned off between them.
  panel.querySelectorAll('button').find(node=>node.dataset.simpleFocus==='move:groq:1').click();await settle();
  assert.deepEqual(requests.at(-1).body.chain.map(entry=>entry.name),['openrouter','groq-20b','groq'],'the next AI in use takes its place');
  // A group opened stays open through the redraws that a poll or a save brings.
  const myWorkGroup=groups()[2];myWorkGroup.open=true;myWorkGroup.isConnected=true;
  for(const cb of myWorkGroup.listeners.toggle||[])cb({target:myWorkGroup});
  ctx.ui.drawSettings();
  assert.equal(groups()[2].open,true,'opened, it stays open');
  ctx.ui.setLang('en');ctx.ui.drawSettings();
  assert.deepEqual(groups().map(titleOf),['What agents are given','AI for summaries','My Work, written by AI','Search by meaning',
    'Privacy','Agents']);
  assert(stateOf(groups()[1]).includes('2 in use')&&stateOf(groups()[1]).includes('$0.39'),stateOf(groups()[1]));
}
console.log('PASS: simple status/models/catalog, remembered view, seven JA/EN blocks, isolated autosaves and masking confirmation');
}
// My Work (X10, decision 55): the tab, the three views, the filters, the request page and the
// new data that waits behind "Show updates" while a person reads.
{
const run=code=>vm.runInContext(code,ctx);
document.querySelector=()=>new Node('main');document.querySelectorAll=()=>[];
ctx.ui.setLang('en');
assert.equal(run('VIEWS[0]'),'my_work','My Work is the first tab');
assert.equal(initialView,'my_work','My Work opens first when nothing is stored');
run("setView('my_work')");
assert.equal(document.getElementById('controls').hidden,true,'My Work hides the global search');
const now=Date.now(),H=3600000;
const req=(id,over={})=>({id,title:`Request ${id}`,summary:`Summary ${id}`,next:null,copy:'recorded',updating:false,status:'working',
  updated:now-H,started:now-5*H,agents:['claude'],sessions:[{agent:'claude',id:`c:claude:${id}`}],projects:['alpha'],
  repos:['github.com/o/alpha'],ask:{id:'dev:1',ts:now-5*H,text:`Ask ${id}`,from:'prompt'},follow_ups:[],
  updates:[{ts:now-H,text:`Summary ${id}`}],work:[],summaries:[],cards:[],...over});
const panel=document.getElementById('panel');
const set=(rows,unlinked=0)=>{run('Object.assign(mw,{rows:null,filter:"all",query:"",project:"",range:"day",sort:"attention",agent:null,request:null,work:null,back:null,pending:null});mw.changed.clear();mw.unread.clear()');
  ctx.__rows={requests:rows,unlinked};run('myWorkAccept(globalThis.__rows);myWorkDraw()');};
const cards=()=>panel.querySelectorAll('article').filter(n=>n.className.startsWith('my-work-card '));
const ids=()=>cards().map(n=>n.dataset.id);
// Cards: the badge, the title, the summary, "Summary updating", the projects, the next line only
// where it says more than the summary (else none), the status's line where none is recorded,
// and the open label by status.
set([req('a',{status:'needs',updating:true,next:'Answer the question.'}),req('b',{status:'ready',next:null}),
  req('c',{next:'Summary c'}),req('d',{summary:null,updates:[],ask:{id:null,ts:now,text:'Ask d',from:'prompt'}})]);
let shown=text(panel);
for(const fragment of ['Needs you','Request a','Summary a','Summary updating','alpha','Answer the question.','Read handoff',
  'Ready to check','Read the reported result and recorded evidence.','Review result','Open request','Ask d']){
  assert(shown.includes(fragment),`card shows: ${fragment}`);
}
const card=id=>cards().find(n=>n.dataset.id===id);
assert(!text(card('c')).includes('Next step'),'a next step the same as the summary is not repeated');
assert(text(card('c')).includes('Follow the latest recorded activity.')===false);
assert(text(card('a')).includes('Claude Code'),'the contributor is named as the page names its agent');
// Attention first: needs, blocked, ready, working, quiet, todo, unknown, closed; ties newest first.
set(['closed','unknown','todo','quiet','working','ready','blocked','needs'].map((status,i)=>req(status,{status,updated:now-(i+1)*60000})));
assert.deepEqual(ids(),['needs','blocked','ready','working','quiet','todo','unknown','closed'],'attention order');
// A list's status (claude-mem 15.1's statusFor): needs review, by any of its names, before
// blocked; `open` in progress; all dropped closed.
const stream=states=>run(`myWorkStreamStatus(${JSON.stringify(states.map(status=>({status,updated:now})))},${now})`);
assert.equal(stream(['failed','Ready for review']),'ready','needs review comes before blocked');
assert.equal(stream(['doing','in_review']),'ready','in review is needs review');
assert.equal(stream(['open']),'working','open is in progress');
assert.equal(stream(['dropped','dropped']),'closed','all dropped is closed');
set([req('x',{updated:now-3*H}),req('y',{updated:now-H}),req('z',{updated:0,title:'A first'})]);
run('mw.range="all";mw.sort="latest";myWorkDraw()');assert.deepEqual(ids(),['y','x','z'],'latest first, no time last');
run('mw.sort="oldest";myWorkDraw()');assert.deepEqual(ids(),['x','y','z'],'oldest first, no time last');
run('mw.sort="title";myWorkDraw()');assert.deepEqual(ids(),['z','x','y'],'A to Z by title');
// P5: the progress line, when it was reviewed or the day it shipped, and the pace's estimate; none
// before a review. Shipped is its own tab, shown with a count, ranked after Checked.
set([req('p1',{progress:{percent:60,reviewed:now-3*H,completed:null,estimate:now+48*H}}),
  req('p2',{status:'shipped',progress:{percent:100,reviewed:now-2*H,completed:'2026-10-20',estimate:null}}),req('p3')]);
// The text as the browser reads it, with no space between nodes.
const flat=node=>typeof node==='string'?node:(node?.children||[]).map(flat).join('');
assert(flat(card('p1')).includes('60% · reviewed 3h ago'),'the percent and when it was reviewed');
run(`myWorkTimes(${now+2*H})`);
assert(flat(card('p1')).includes('60% · reviewed 5h ago'),'the review\'s age follows the clock');
assert(text(card('p1')).includes('At this pace, about'),'the estimate');
assert.equal(card('p1').querySelectorAll('meter')[0].value,60,'the meter holds the percent');
assert(text(card('p2')).includes('Completed Oct 20'),'the day it shipped');
assert(text(card('p2')).includes('Shipped'),'the shipped badge');
assert(!text(card('p3')).includes('%'),'no progress before a review');
assert.deepEqual(ids(),['p1','p3','p2'],'shipped after the others');
const shippedTab=panel.querySelectorAll('button').find(n=>n.dataset.myWork==='tab:shipped');
assert.equal(text(shippedTab),'Shipped 1','Shipped shows with its count');
// A check of a result before it shipped: Shipped stays, with its day.
set([req('ck',{status:'shipped',checked:now-H,progress:{percent:100,reviewed:now-H,completed:'2026-10-20',estimate:null}})]);
assert(text(card('ck')).includes('Shipped')&&!text(card('ck')).includes('Checked'),'a shipped request stays Shipped though checked');
assert(text(card('ck')).includes('Completed Oct 20'),'with the day it shipped');
// The request's page: the progress and the build story, the milestones the oldest first.
set([req('st',{progress:{percent:40,reviewed:now-H,completed:null,estimate:null},
  story:{narrative:'The parser came first.',milestones:[{date:'2026-10-02',text:'Reads a line.'},{date:'2026-10-05',text:'Tests pass.'}]}})]);
run('myWorkOpen("st")');
const page=text(panel);
for(const fragment of ['40% · reviewed','Build story','The parser came first.','Oct 2','Reads a line.','Oct 5','Tests pass.']){
  assert(page.includes(fragment),`the request's page shows: ${fragment}`);
}
assert(page.indexOf('Oct 2')<page.indexOf('Reads a line.')&&page.indexOf('Reads a line.')<page.indexOf('Oct 5')&&
  page.indexOf('Oct 5')<page.indexOf('Tests pass.'),'the milestones the oldest first, each after its day');
assert(!page.includes('2026-10-02'),"a milestone's day as claude-mem's work view shows it");
run('myWorkClose()');
ctx.ui.setLang('ja');
set([req('pj',{progress:{percent:70,reviewed:now-2*H,completed:null,estimate:null}})]);
assert(flat(card('pj')).includes('70% ・2 時間前に確認'),'the Japanese progress line');
ctx.ui.setLang('en');
// Status tabs: the six always there, Blocked only with one, counts of the scope, a click filters
// and names the heading.
set([req('w1'),req('w2'),req('n1',{status:'needs'})]);
const tabs=()=>panel.querySelectorAll('button').filter(n=>n.dataset.myWork?.startsWith('tab:'));
assert.deepEqual(tabs().map(n=>n.dataset.myWork),['tab:all','tab:needs','tab:working','tab:quiet','tab:ready','tab:checked']);
assert.deepEqual(tabs().map(n=>text(n)),['All 3','Needs you 1','In progress 2','No recent update 0','Ready to check 0','Checked 0']);
set([req('w1'),req('b1',{status:'blocked'})]);
assert(tabs().some(n=>n.dataset.myWork==='tab:blocked'),'Blocked shows with a count');
tabs().find(n=>n.dataset.myWork==='tab:blocked').click();
assert.deepEqual(ids(),['b1'],'a tab filters');
assert.equal(text(tabs()[0]),'All 2','the counts stay those of the scope, before the tab');
assert(text(panel.querySelectorAll('h3').find(n=>n.className==='my-work-heading')).includes('Blocked'),'the heading names the tab');
assert.equal(document.activeElement.dataset.myWork,'tab:blocked','the tab keeps the focus');
// Search: one substring of title, summary, agent and projects; project and range filters.
set([req('s1',{title:'Fix the parser',projects:['alpha']}),req('s2',{title:'Docs',summary:'progress',projects:['beta']}),
  req('s3',{title:'Old',updated:now-25*H}),req('s4',{title:'progress report',projects:['alpha']})]);
const field=()=>panel.querySelectorAll('input').find(n=>n.dataset.myWork==='search');
const typed=field();typed.value='parser';for(const cb of typed.listeners.input)cb({target:typed});
assert.equal(field(),typed,'typing keeps the search field, so an IME\'s text being composed is not cut off');
assert.deepEqual(ids(),['s1'],'what is typed searches');
run('mw.query="claude code";myWorkDraw()');assert.deepEqual(ids().sort(),['s1','s2','s4'],'the agent is searched by its shown name');
run('mw.query="progress alpha";myWorkDraw()');assert.deepEqual(ids(),[],'one substring, not words, though s4 has both');
assert(text(panel).includes('No matching work'));
run('mw.query="";mw.project="beta";myWorkDraw()');assert.deepEqual(ids(),['s2'],'project filter');
run('mw.project="";myWorkDraw()');assert(!ids().includes('s3'),'the past 24 hours hide a request of 25 hours ago');
run('mw.range="all";myWorkDraw()');assert(ids().includes('s3'),'all recorded work shows it');
const project=panel.querySelectorAll('select').find(n=>n.dataset.myWork==='project');
assert.deepEqual(project.options.map(o=>o.value),['','alpha','beta'],'every project, sorted, after All projects');
// List: five columns, the next step or the summary in the third; open by the title.
set([req('l1',{next:'Do the next thing'}),req('l2',{next:'Summary l2'}),req('l3')]);
run('mw.layout="list";myWorkDraw()');
const table=panel.querySelector('table');
assert.equal(table.querySelectorAll('th').length,5,'five columns');
const rowsOf=()=>table.querySelectorAll('tr').filter(n=>n.dataset.id);
assert(text(rowsOf()[0]).includes('Do the next thing')&&text(rowsOf()[1]).includes('Summary l2'),'the next step, else the summary');
assert(text(rowsOf()[2]).includes('Follow the latest recorded activity.'),'the status\'s line where none is recorded');
// Agents: one card a conversation (decision 55), two of one agent apart; choosing one shows its
// requests with "← All agents".
set([req('g1',{sessions:[{agent:'claude',id:'c:claude:s1'}],updated:now-60000}),
  req('g2',{sessions:[{agent:'claude',id:'c:claude:s2'}],updated:now-120000}),
  req('g3',{sessions:[{agent:'claude',id:'c:claude:s1'}],updated:now-180000,title:'Older one'}),
  req('g4',{sessions:[]})]);
run('mw.layout="agents";myWorkDraw()');
const agents=()=>panel.querySelectorAll('article').filter(n=>n.className.includes('my-work-agent'));
assert.deepEqual(agents().map(n=>n.dataset.session),['c:claude:s1','c:claude:s2'],'one card a conversation');
assert(text(agents()[0]).includes('2 human requests')&&text(agents()[0]).includes('Request g1'),'its count and latest title');
agents()[0].querySelector('button').click();
assert.deepEqual(ids(),['g1','g3'],'the conversation\'s requests');
assert(text(panel).includes('← All agents'));
panel.querySelectorAll('button').find(n=>n.dataset.myWork==='all-agents').click();
assert.equal(agents().length,2,'back to every conversation');
run('mw.layout="cards"');
// The request page: facts, where it stands, the original, the work by repository and list with
// its status, the follow-ups, the recorded result and cards; back returns the focus to the card.
const masked='c:[REDACTED]#0123456789ab';
const alpha='github.com/o/alpha',beta='github.com/o/beta';
set([req(masked,{status:'ready',next:null,projects:['alpha','beta'],repos:[alpha,beta],work:[
  {list:'release',repo:alpha,task:'tag',status:'done',detail:'Tagged.',updated:now-H},
  {list:'release',repo:alpha,task:'notes',status:'done',detail:null,updated:now-2*H},
  {list:'docs',repo:alpha,task:'guide',status:'doing',detail:null,updated:now-H},
  {list:'docs',repo:beta,task:'tutorial',status:'todo',detail:null,updated:now-H}],
  follow_ups:[{id:'dev:2',ts:now,text:'And the column.'}],summaries:['S1'],cards:['9.0']}),req('other')]);
const opener=card(masked).querySelector('button');opener.click();
assert.equal(text(document.getElementById('heading')),`Request ${masked}`,'the request page is titled');
shown=text(panel);
for(const fragment of ['← Back to My Work','Contributors','Started','Elapsed','Where it stands','Ready to check:',
  'Read the reported result and recorded evidence.','Original request','Ask','Work behind this request','release','Tagged.',
  'docs','guide','Follow-ups to this request','And the column.','Recorded result','S1','Recorded cards','9.0','Timeline →']){
  assert(shown.includes(fragment),`request page shows: ${fragment}`);
}
for(const absent of ['Stage','Ship estimate','Requested by','After ship','Time to completion']){
  assert(!shown.includes(absent),`no row for what oboete does not record: ${absent}`);
}
const streamsOf=()=>panel.querySelectorAll('article').filter(n=>n.className==='my-work-stream');
const streams=streamsOf();
assert.deepEqual(streams.map(n=>text(n.querySelector('button')).replace(/\s+/g,' ')),['release alpha','docs alpha','docs beta'],
  'the work by repository and list, each list\'s button naming its repository (#600)');
assert.deepEqual(streams.map(n=>text(n.querySelector('.my-work-project'))),['alpha','alpha','beta'],'each list names its repository when the work is in two');
assert(text(streams[0]).includes('Ready to check')&&text(streams[1]).includes('In progress'),'each list by claude-mem\'s statusFor');
streams[0].querySelector('button').click();
shown=text(panel);
assert(shown.includes('← Back to request')&&shown.includes('Recorded changes')&&shown.includes('notes'),'the work stream page');
panel.querySelectorAll('button').find(n=>text(n)==='← Back to request').click();
assert(text(panel).includes('Where it stands'),'back to the request');
assert.equal(document.activeElement,streamsOf()[0].querySelector('button'),'back returns the focus to the list that opened it');
streamsOf()[2].querySelector('button').click();
shown=text(panel);
assert(shown.includes('tutorial')&&!shown.includes('guide'),'a list of one name in another repository is a stream of its own');
assert.equal(text(panel.querySelector('.my-work-request-meta').querySelector('.my-work-project')),'beta','and its page names its repository');
panel.querySelectorAll('button').find(n=>text(n)==='← Back to request').click();
assert.equal(document.activeElement,streamsOf()[2].querySelector('button'),'the focus returns to that one, not its namesake');
panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
assert(document.activeElement?.dataset.id===masked,'back returns the focus to the card that opened it, a masked id too');
// Slice 4: a request's work product, read each time its page opens and once a version while it stays open; a link only for
// http and https, opening apart from the page; a path shown as text; an empty answer, no section.
{
const asked=[];let products=[];
ctx.fetch=url=>{asked.push(url);return Promise.resolve(response({products}));};
const box=()=>panel.querySelector('.my-work-products');
const back=()=>panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
products=[{kind:'commit',title:'abc1234 Fix the parser',url:'https://github.com/o/r/commit/abc1234',path:null,at:now-H},
  {kind:'document',title:'notes.pdf',url:null,path:'/work/notes.pdf',at:null},
  {kind:'link',title:'Not a link',url:'javascript:alert(1)',path:null,at:null}];
set([req('p1',{version:'v1'})]);
card('p1').querySelector('button').click();
assert(box().hidden,'the section waits for its answer');
await new Promise(resolve=>setImmediate(resolve));
assert.deepEqual(asked,['/api/my-work/product?request=p1'],'the page asks for its request');
const shownProducts=text(box());
assert(!box().hidden&&shownProducts.includes('Work product'),'the section shows what was read');
const links=box().querySelectorAll('a');
assert.equal(links.length,1,'only an http or https address is a link');
assert.equal(links[0].href,'https://github.com/o/r/commit/abc1234');
assert.equal(links[0].target,'_blank');assert.equal(links[0].rel,'noopener noreferrer');
for(const fragment of ['Commit','Document','notes.pdf','/work/notes.pdf','Not a link']){
  assert(shownProducts.includes(fragment),`the work product shows: ${fragment}`);
}
run('myWorkDraw()');
assert(asked.length===1&&!box().hidden,'a redraw of the open page shows what was read, not read again');
back();
card('p1').querySelector('button').click();
assert(box().hidden,'opened again, the page reads again: records and rules change it, not the version (Codex on #650)');
await new Promise(resolve=>setImmediate(resolve));
assert(asked.length===2&&!box().hidden,'and shows the new answer');
back();
products=[];
set([req('p2')]);
card('p2').querySelector('button').click();
await new Promise(resolve=>setImmediate(resolve));
assert.equal(asked.length,3,'another request is read');
assert(box().hidden,'nothing read, no section');
back();
// An earlier opening's answer that arrives after a later one's is not kept (CodeRabbit on #650).
const held=[];
ctx.fetch=url=>{asked.push(url);const list=products;return new Promise(resolve=>held.push(()=>resolve(response({products:list}))));};
set([req('p3',{version:'v1'})]);
products=[{kind:'commit',title:'Earlier answer',url:null,path:null,at:null}];
card('p3').querySelector('button').click();
back();
products=[{kind:'commit',title:'Later answer',url:null,path:null,at:null}];
card('p3').querySelector('button').click();
held[1]();await new Promise(resolve=>setImmediate(resolve));
held[0]();await new Promise(resolve=>setImmediate(resolve));
run('myWorkDraw()');
assert(text(box()).includes('Later answer')&&!text(box()).includes('Earlier answer'),'the later answer stays');
back();
}
// New data waits while a person reads (focus in the toolbar, or a request open), behind "Show
// updates", and an unchanged answer does nothing.
set([req('h1')]);
let answer={requests:[req('h1',{title:'Changed title'})],unlinked:0};
ctx.fetch=url=>{assert(url.startsWith('/api/my-work?range=all'));return Promise.resolve(response(answer));};
document.activeElement=panel.querySelectorAll('input').find(n=>n.dataset.myWork==='search');
await run('myWorkLive()');
assert(!text(panel).includes('Changed title'),'the cards stay while a person reads');
assert(text(panel).includes('New work updates are ready.'),'the banner says so');
panel.querySelectorAll('button').find(n=>text(n)==='Show updates').click();
assert(text(panel).includes('Changed title')&&!text(panel).includes('New work updates are ready.'),'Show updates applies them');
document.activeElement=null;
const before=panel.children;await run('myWorkLive()');
assert.equal(panel.children,before,'an unchanged answer draws nothing');
answer={requests:[req('h1',{title:'Changed again'})],unlinked:2};
await run('myWorkLive()');
assert(text(panel).includes('Changed again')&&text(panel).includes('2 work records need a linked human request'),'applied when no one reads');
// What is held is the last answer received, as claude-mem's `accept` keeps it: the same answer
// again stays held after the reading ends, and one like the shown one replaces it, so that
// "Show updates" never applies an answer older than the last.
const reading=()=>{document.activeElement=panel.querySelectorAll('input').find(n=>n.dataset.myWork==='search');};
set([req('k1')]);
answer={requests:[req('k1',{title:'Held title'})],unlinked:0};
reading();await run('myWorkLive()');
document.activeElement=null;await run('myWorkLive()');
assert(!text(panel).includes('Held title')&&text(panel).includes('New work updates are ready.'),'the same answer again stays held');
answer={requests:[req('k1')],unlinked:0};
reading();await run('myWorkLive()');
panel.querySelectorAll('button').find(n=>text(n)==='Show updates').click();
assert(!text(panel).includes('Held title'),'an answer like the shown one replaces the held one');
answer={requests:[req('k1',{title:'Held title'})],unlinked:0};
reading();await run('myWorkLive()');
answer={requests:[req('k1')],unlinked:0};
document.activeElement=null;await run('myWorkLive()');
assert(!text(panel).includes('Held title')&&!text(panel).includes('New work updates are ready.'),'and with no one reading, nothing stays held');
// A poll's read that a newer drawing overtook (`show`) draws nothing.
set([req('o1')]);
let release;
ctx.fetch=()=>new Promise(resolve=>{release=()=>resolve(response({requests:[req('o1',{title:'Stale title'})],unlinked:0}));});
const late=run('myWorkLive()');
run('generation++');release();await late;
assert(!text(panel).includes('Stale title')&&!text(panel).includes('New work updates are ready.'),'an overtaken read draws nothing');
// The tab's read and a poll's read in flight together: the newer answer stays, whichever comes
// last, and the tab's drawing still draws the page (Codex and Greptile on #597).
set([req('q1')]);
const releases=[];
ctx.fetch=()=>new Promise(resolve=>releases.push(title=>resolve(response({requests:[req('q1',{title})],unlinked:0}))));
document.activeElement=null;
const load=run('showMyWork()'),polled=run('myWorkLive()');
releases[1]('Newer poll');await polled;
releases[0]('Older load');(await load)();
assert(text(panel).includes('Newer poll')&&!text(panel).includes('Older load'),'an older read never replaces a newer one');
// The slot of the held update is a polite live region (CodeRabbit on #597).
assert.equal(panel.querySelector('.my-work-updates').attrs.role,'status','a screen reader says an update is held');
// A changed request flashes once and keeps "New update" until it is opened, as claude-mem marks
// a changed card (`apply`, `markRead`); one whose summary alone a model rewrote flashes and is not
// marked; a request seen unchanged is marked neither way.
set([req('u1'),req('u2'),req('u3')]);
answer={requests:[req('u1',{updates:[{ts:now,text:'A new update'}],summary:'A new update'}),req('u2',{summary:'Rewritten'}),req('u3')],unlinked:0};
ctx.fetch=()=>Promise.resolve(response(answer));
await run('myWorkLive()');
const flashed=id=>card(id).className.includes('my-work-updated'),marked=id=>text(card(id)).includes('New update');
assert.deepEqual(['u1','u2','u3'].map(flashed),[true,true,false],'a changed card flashes');
assert.deepEqual(['u1','u2','u3'].map(marked),[true,false,false],'one whose record changed is marked new');
run('myWorkDraw()');
assert(!card('u1').className.includes('my-work-updated'),'the flash is once');
assert(text(card('u1')).includes('New update'),'the mark stays until the request is opened');
card('u1').querySelector('button').click();
panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
assert(!text(card('u1')).includes('New update'),'opening the request reads it');
// Times are rewritten on each poll's tick, as claude-mem rewrites them each second.
set([req('t1',{updated:now-2*60000})]);
const timeOf=()=>text(card('t1').querySelector('time'));
assert.equal(timeOf(),'2m ago');
run(`myWorkTimes(${now}+3*3600000)`);
assert.equal(timeOf(),'3h ago','the card\'s time follows the clock');
card('t1').querySelector('time').dataset.time=String(now-5*3600000);
document.visibilityState='visible';
ctx.fetch=url=>Promise.resolve(response(url.startsWith('/api/version')?{v:run('version')}:{requests:[req('t1',{updated:now-2*60000})],unlinked:0}));
await run('poll()');
delete document.visibilityState;
assert.equal(timeOf(),'5h ago','each poll\'s tick rewrites the times');
card('t1').querySelector('button').click();
assert(!text(panel).includes('Mark checked'),'a request not reported complete has no review');
const elapsedOf=()=>text(panel.querySelector('.my-work-elapsed'));
assert.equal(elapsedOf(),'5h 0m');
run(`myWorkTimes(${now}+3600000)`);
assert.equal(elapsedOf(),'6h 0m','and so does the request\'s elapsed time');
// Lists the gate masks alike stay two by their fingerprints (CodeRabbit on #597).
set([req('f1',{work:[{list:'[REDACTED]',repo:'[REDACTED]',stream:'aaaaaaaaaaaa',task:'one',status:'doing',detail:null,updated:now-H},
  {list:'[REDACTED]',repo:'[REDACTED]',stream:'bbbbbbbbbbbb',task:'two',status:'doing',detail:null,updated:now-H}]})]);
card('f1').querySelector('button').click();
assert.equal(streamsOf().length,2,'two lists masked alike are two streams');
assert.equal(streamsOf()[0].querySelectorAll('.my-work-project').length,0,'a repository masked alike is not named apart');
// Copy handoff, in the page's language (decision 55), as claude-mem's `handoffText`; without a
// clipboard, the text selected in a field to copy.
set([req('c1',{updates:[{ts:now,text:'Parser fixed.'},{ts:now-1,text:'Tests added.'}]})]);
card('c1').querySelector('button').click();
let copied=null;
ctx.navigator.clipboard={writeText:value=>{copied=value;return Promise.resolve();}};
const copy=()=>panel.querySelectorAll('button').find(n=>n.className.includes('my-work-copy'));
copy().click();await new Promise(r=>setTimeout(r,0));
assert.equal(copied,['Request: Ask c1','Project: alpha','Recorded state: In progress','Next step: Follow the latest recorded activity.','',
  'Latest recorded updates:','- Parser fixed.','- Tests added.','','Continue in the original work conversation. This handoff does not assign or send work.'].join('\n'));
assert(text(panel).includes('Handoff copied. Paste it into the agent or conversation you choose.'),'the copy is confirmed');
ctx.ui.setLang('ja');run('myWorkDraw()');
copy().click();await new Promise(r=>setTimeout(r,0));
assert(copied.startsWith('依頼: Ask c1\nプロジェクト: alpha\n記録された状態: 進行中\n'),`the handoff in Japanese: ${copied}`);
ctx.ui.setLang('en');run('myWorkDraw()');
// A state the page names is the handoff's too, where claude-mem says "Recorded update" (a local
// Codex review of slice 2's last PR).
for(const [status,said] of [['blocked','Blocked'],['quiet','No recent update'],['todo','To do'],['closed','Closed'],['unknown','Recorded update']]){
  run(`mw.rows.find(r=>r.id==='c1').status='${status}';myWorkDraw()`);
  copy().click();await new Promise(r=>setTimeout(r,0));
  assert.equal(copied.split('\n')[2],`Recorded state: ${said}`,status);
}
run("mw.rows.find(r=>r.id==='c1').status='working';myWorkDraw()");
delete ctx.navigator.clipboard;
copy().click();await new Promise(r=>setTimeout(r,0));
const area=panel.querySelector('textarea');
assert(!area.hidden&&area.selected&&area.value.startsWith('Request: Ask c1'),'without a clipboard the text waits selected');
assert(text(panel).includes('Clipboard unavailable. Copy the selected handoff text below.'));
panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
// Checked (docs/my-work.md rule 6): a checked request is Checked on the page, counted by its tab
// and ordered last; one reported complete opens to its review, whose "Mark checked" posts the
// version the page read, then reads My Work again; Undo takes the check back; a version that
// changed meanwhile is refused, read again and said so.
const posted=(status,value={})=>({ok:status>=200&&status<300,status,headers:{get:()=>'application/json'},json:async()=>value});
const ticks=async()=>{for(let i=0;i<6;i++)await new Promise(r=>setTimeout(r,0));};
const V='a'.repeat(64);
set([req('k1',{status:'ready',version:V,checked:now-60000}),req('k2',{status:'ready',version:V})]);
run('mw.range="all";myWorkDraw()');
assert.deepEqual(ids(),['k2','k1'],'Checked last in attention order');
assert(text(card('k1')).includes('Checked')&&text(card('k1')).includes('Open request'),'a checked request is Checked');
assert(text(card('k2')).includes('Ready to check')&&text(card('k2')).includes('Review result'));
assert.equal(text(tabs().find(n=>n.dataset.myWork==='tab:checked')),'Checked 1');
let sent=[],checkedNow=true,refuse=false;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){sent.push([url,JSON.parse(options.body)]);return Promise.resolve(refuse?posted(409,{code:'stale'}):posted(200));}
  return Promise.resolve(response({requests:[req('k2',{status:'ready',version:V,...(checkedNow?{checked:now}:{})})],unlinked:0}));
};
run(`Object.assign(mw.rows.find(r=>r.id==='k2'),{updates:[{ts:${now},text:'Tests pass.'},{ts:${now-1},text:'Parser fixed.'}]})`);
card('k2').querySelector('button').click();
shown=text(panel);
for(const fragment of ['Recorded result · your review','Recorded evidence','Tests pass.','Parser fixed.','Checking saves a local note','Mark checked']){
  assert(shown.includes(fragment),`the review shows: ${fragment}`);
}
assert(!shown.includes('No separate test evidence was recorded.'),'the recorded updates are its evidence');
const button=label=>panel.querySelectorAll('button').find(n=>text(n)===label);
button('Mark checked').click();await ticks();
assert.deepEqual(sent,[['/api/my-work/review',{request:'k2',version:V,checked:true}]],'the check names the version read');
assert(button('Undo check')&&!button('Mark checked')&&text(panel).includes('Checked locally'),'the request reads checked');
// #602: the time of the check as text, in the label and in its variables.
const checkedAt=panel.querySelector('.my-work-checked-at');
assert.equal(text(checkedAt),`Checked locally ${new Date(now).toLocaleString('en')}`);
assert.equal(typeof JSON.parse(checkedAt.dataset.i18nVars).time,'string','a switch of language keeps the time');
assert.equal(document.getElementById('status').textContent,'Checked locally. Your review is saved.');
checkedNow=false;
button('Undo check').click();await ticks();
assert.deepEqual(sent[1],['/api/my-work/review',{request:'k2',version:V,checked:false}],'Undo takes the check back');
assert(button('Mark checked'),'ready to check again');
assert.equal(document.getElementById('status').textContent,'Check undone. The result is ready to review.');
refuse=true;
button('Mark checked').click();await ticks();
assert.equal(document.getElementById('status').textContent,'This work changed while you were reading. Review the latest version before checking.');
refuse=false;
// #602: a save the server took is said as taken though the read after it fails.
const kept=ctx.fetch;
ctx.fetch=(url,options)=>options?.method==='POST'?Promise.resolve(posted(200)):Promise.resolve({ok:false,status:503,json:async()=>({})});
button('Mark checked').click();await ticks();
assert.equal(document.getElementById('status').textContent,'Saved. The page shows it at its next read.');
ctx.fetch=kept;
// The read after a check outruns a poll's that began before it: the poll's older answer changes
// nothing, and waits behind no "Show updates".
let releasePoll=null,checkedPoll=0;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST')return Promise.resolve(posted(200));
  if(!checkedPoll++)return new Promise(r=>{releasePoll=()=>r(response({requests:[req('k2',{status:'ready',version:V,title:'Stale poll'})],unlinked:0}));});
  return Promise.resolve(response({requests:[req('k2',{status:'ready',version:V,checked:now})],unlinked:0}));
};
const slowPoll=run('myWorkLive()');
button('Mark checked').click();await ticks();
releasePoll();await slowPoll;
assert(button('Undo check')&&!text(panel).includes('Stale poll')&&!text(panel).includes('New work updates are ready.'),'an older poll read changes nothing');
// And a poll's read that began after the read after a check, and came first, is not undone by it:
// held, as the person reads, and still held when the older read lands.
let releaseReread=null,polledAfter=null,reads=0;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST')return Promise.resolve(posted(200));
  if(!reads++)return new Promise(r=>{releaseReread=()=>r(response({requests:[req('k2',{status:'ready',version:V,title:'Older reread'})],unlinked:0}));});
  return Promise.resolve(response({requests:[req('k2',{status:'ready',version:V,checked:now,title:'Newer poll'})],unlinked:0}));
};
button('Undo check').click();await ticks();
polledAfter=run('myWorkLive()');await polledAfter;
releaseReread();await ticks();
assert(text(panel).includes('New work updates are ready.')&&!text(panel).includes('Older reread'),'the newer answer stays held');
panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
set([req('k3',{status:'ready',version:V,updates:[]})]);
card('k3').querySelector('button').click();
assert(text(panel).includes('No separate test evidence was recorded.'),'with no update, the line for none');
panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
// #600: a request after an empty first answer flashes and is marked new.
set([]);
answer={requests:[req('e1')],unlinked:0};
ctx.fetch=()=>Promise.resolve(response(answer));
document.activeElement=null;await run('myWorkLive()');
assert(card('e1').className.includes('my-work-updated')&&text(card('e1')).includes('New update'),'an empty first answer is a baseline too');
// J4: the request of a conversation the judge suggested shows "Looks like part of <title>" with
// Join and Not this on its card, its row and its page; each posts its answer, reads My Work again
// and says so; a Join from the request's page shows the request it joined; a refusal is said.
{
  const S={id:'p1',title:'Parser',conversation:{id:'c:claude:s2'}};
  const rows=()=>[req('p1',{title:'Parser'}),req('s2',{title:'Trailing comma',suggestion:S})];
  // After a Join, the request that holds the conversation: Parser, or (#775) the one it went to.
  const both=(id,title)=>req(id,{title,sessions:[{agent:'claude',id:`c:claude:${id}`},{agent:'claude',id:'c:claude:s2'}]});
  let sent=[],refuse=null,joined=null,unread=false;
  ctx.fetch=(url,options)=>{
    if(options?.method==='POST'){
      sent.push([url,JSON.parse(options.body)]);
      return Promise.resolve(refuse?posted(refuse[0],{code:refuse[1]}):posted(200,{state:'applied'}));
    }
    if(unread)return Promise.resolve({ok:false,status:503,json:async()=>({})});
    return Promise.resolve(response({requests:joined?[joined].flat():rows(),unlinked:0}));
  };
  set(rows());run('mw.range="all";myWorkDraw()');
  const line=node=>node.querySelector('.my-work-suggestion');
  const answerOf=(node,action)=>node.querySelectorAll('button').find(n=>n.dataset.action===`my-work.suggestion.${action}`);
  const label=node=>node.getAttribute?.('aria-label')??node.attrs['aria-label'];
  assert(!line(card('p1')),'a request with no suggestion shows none');
  assert.equal(text(line(card('s2')).querySelector('p')),'Looks like part of “Parser”.');
  assert.deepEqual([label(answerOf(card('s2'),'join')),label(answerOf(card('s2'),'not-this'))],['Join: Parser','Not this: Parser']);
  // While an answer is on its way, the answers' buttons wait (#775).
  {
    let release=null;
    const kept=ctx.fetch;
    ctx.fetch=(url,options)=>options?.method==='POST'?new Promise(r=>{release=()=>r(posted(200,{state:'applied'}));}):kept(url,options);
    answerOf(card('s2'),'not-this').click();
    const waiting=panel.querySelectorAll('.my-work-answer');
    assert(waiting.length===2&&waiting.every(b=>b.disabled),'the buttons wait');
    release();await ticks();
    ctx.fetch=kept;
  }
  answerOf(card('s2'),'not-this').click();await ticks();
  assert.deepEqual(sent,[['/api/my-work/suggestion',{request:'s2',action:'not-this'}]],'Not this names the request');
  assert.equal(document.getElementById('status').textContent,'Kept as a request of its own.');
  run('mw.layout="list";myWorkDraw()');
  const tableRow=panel.querySelectorAll('tr').find(n=>n.dataset.id==='s2');
  assert(line(tableRow)&&answerOf(tableRow,'join'),'the list row has it too');
  run('mw.layout="cards";myWorkDraw()');
  card('s2').querySelector('button').click();
  assert(answerOf(panel,'join'),'the request page has it');
  joined=both('p1','Parser');
  answerOf(panel,'join').click();await ticks();
  assert.deepEqual(sent[1],['/api/my-work/suggestion',{request:'s2',action:'join'}],'Join names the request');
  assert.equal(run('mw.request'),'p1','the request it joined is shown');
  assert.equal(document.getElementById('status').textContent,'Joined to “Parser”.');
  // #777 (an_unchanged_poll_after_an_answer_does_not_offer_show_updates): the poll after it reads
  // the answer the page read after the Join, which is no update.
  await run('myWorkLive()');
  assert(!run('mw.pending')&&!text(panel).includes('New work updates are ready.'),'the same answer is no update');
  panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
  // #775 (join_opens_the_current_destination_after_the_pick_moves): Parser joined Reader after the
  // page drew it; the Join landed in Reader, which the page shows and names.
  joined=null;set(rows());card('s2').querySelector('button').click();
  joined=both('r1','Reader');
  answerOf(panel,'join').click();await ticks();
  assert.deepEqual([run('mw.request'),document.getElementById('status').textContent],['r1','Joined to “Reader”.']);
  panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
  // Codex on #776 (join_without_a_loaded_destination_does_not_name_the_old_pick): the read after a
  // Join holds no request with the conversation; the page goes back to the list and says it was
  // saved, naming no request.
  joined=null;set(rows());card('s2').querySelector('button').click();
  joined=req('x1',{title:'Unrelated'});
  answerOf(panel,'join').click();await ticks();
  assert.deepEqual([run('mw.request'),document.getElementById('status').textContent],[null,'Saved. The page shows it at its next read.']);
  // #777 (join_follows_the_suggested_anchor_after_a_members_grouping_is_forgotten): s2's request,
  // drawn with s3 active since, which has left it by the Join; the Join put s2 into Parser, which
  // the page shows and names, not s3's request of its own.
  joined=null;set([req('p1',{title:'Parser'}),req('s2',{title:'Trailing comma',suggestion:S,
    sessions:[{agent:'claude',id:'c:claude:s3'},{agent:'claude',id:'c:claude:s2'}]})]);
  card('s2').querySelector('button').click();
  joined=[both('p1','Parser'),req('s3',{title:'Commas'})];
  answerOf(panel,'join').click();await ticks();
  assert.deepEqual([run('mw.request'),document.getElementById('status').textContent],['p1','Joined to “Parser”.']);
  panel.querySelectorAll('button').find(n=>text(n)==='← Back to My Work').click();
  // #775 (suggestion_reread_failure_restores_controls): the read after an answer fails; what was
  // saved is said so, and every suggestion's buttons can be used again.
  joined=null;set([...rows(),req('s3',{title:'Another',suggestion:S})]);unread=true;
  answerOf(card('s2'),'not-this').click();await ticks();
  unread=false;
  assert.equal(document.getElementById('status').textContent,'Saved. The page shows it at its next read.');
  const answers=panel.querySelectorAll('.my-work-answer');
  assert(answers.length===4&&answers.every(b=>!b.disabled),'the buttons are usable again');
  joined=null;refuse=[404,'suggestion_not_found'];set(rows());
  answerOf(card('s2'),'join').click();await ticks();
  assert.equal(document.getElementById('status').textContent,'That suggestion is no longer waiting for an answer. The page has been read again.');
  refuse=null;
  ctx.ui.setLang('ja');run('myWorkDraw()');
  assert.equal(text(line(card('s2')).querySelector('p')),'「Parser」の一部のようです。');
  assert.deepEqual([text(answerOf(card('s2'),'join')),text(answerOf(card('s2'),'not-this'))],['まとめる','違う']);
  ctx.ui.setLang('en');run('mw.layout="cards";myWorkDraw()');
}
// Every My Work label in both languages, each language its own words.
const labels=Object.entries(run('TEXT')).filter(([key])=>key.startsWith('my_work'));
assert(labels.length>100,'My Work has its labels');
for(const [key,[en,ja]] of labels)assert(en&&ja&&en!==ja,`${key} has distinct English and Japanese`);
ctx.ui.setLang('ja');set([req('j1',{status:'needs'})]);
assert(text(panel).includes('対応が必要')&&text(panel).includes('マイワーク')===false,'the page draws in Japanese');
assert.equal(text(document.getElementById('heading')),'マイワーク','its heading too');
ctx.ui.setLang('en');
console.log('PASS: My Work: first tab, cards, attention, tabs, search, list, one agent card a conversation, the request page, back focus, held updates, the order of reads, the marks of a change, the times, the work product, Copy handoff and Checked, in both languages');
}

// Decision 61 slice 10c-1 (docs/my-work-model.md F7): My Focus's chips, the area filter, the
// card's area, and Move to… with its change bar and Undo, in both languages.
{
const run=code=>vm.runInContext(code,ctx);
document.querySelector=()=>new Node('main');document.querySelectorAll=()=>[];
ctx.ui.setLang('en');
const now=Date.now(),H=3600000;
const req=(id,over={})=>({id,title:`Request ${id}`,summary:`Summary ${id}`,next:null,copy:'recorded',updating:false,status:'working',
  updated:now-H,started:now-5*H,agents:['claude'],sessions:[{agent:'claude',id:`c:claude:${id}`}],projects:['alpha'],
  repos:['github.com/o/alpha'],ask:{id:'dev:1',ts:now-5*H,text:`Ask ${id}`,from:'prompt'},follow_ups:[],
  updates:[{ts:now-H,text:`Summary ${id}`}],work:[],summaries:[],cards:[],focus:null,...over});
const response=value=>({ok:true,status:200,json:async()=>value});
const posted=(status,value={})=>({ok:status>=200&&status<300,status,headers:{get:()=>'application/json'},json:async()=>value});
const ticks=async()=>{for(let i=0;i<6;i++)await new Promise(r=>setTimeout(r,0));};
const panel=document.getElementById('panel');
const P='fa_000000000001',D='fa_000000000002',L='fa_000000000003';
const areas=[{id:P,name:'Parser',status:'kept'},{id:D,name:'Docs',status:'kept'},{id:L,name:'Lexer',status:'suggested'}];
run("setView('my_work')");
const set=(rows,list=areas)=>{run('Object.assign(mw,{rows:null,filter:"all",query:"",project:"",range:"all",sort:"attention",agent:null,request:null,work:null,back:null,pending:null,area:null,saved:null});mw.changed.clear();mw.unread.clear()');
  ctx.__rows={requests:rows,unlinked:0,areas:list};run('myWorkAccept(globalThis.__rows);myWorkDraw()');};
const cards=()=>panel.querySelectorAll('article').filter(n=>n.className.startsWith('my-work-card '));
const ids=()=>cards().map(n=>n.dataset.id).sort();
const card=id=>cards().find(n=>n.dataset.id===id);
const chips=()=>panel.querySelectorAll('button').filter(n=>n.dataset.myWork?.startsWith('chip:'));
// Not in use (no area, no placement): nothing of it is drawn.
set([req('n1')],[]);
assert.equal(chips().length,0,'no chips while My Focus is not in use');
assert(!text(panel).includes('Sorting…'),'no card says Sorting…');
set([req('f1',{focus:P,status:'needs'}),req('f2',{focus:P}),req('f3',{focus:'other',status:'ready'}),req('f4')]);
assert.deepEqual(chips().map(n=>n.dataset.myWork),[`chip:${P}`,`chip:${D}`,'chip:other','chip:suggested','chip:past-focus'],'the kept areas in order, then Other, the suggestions and Past focus');
assert.deepEqual(chips().slice(3).map(n=>text(n)),['1 suggested','Past focus']);
assert.equal(text(chips()[0]),'Parser 1 1 need you','with how many need you, said in words');
assert.equal(chips()[0].querySelector('.my-work-count').attrs['aria-hidden'],'true','the number alone is not read');
assert.equal(text(chips()[1]),'Docs','none at 0');
assert.equal(text(chips()[2]),'Other 1 1 need you','one ready to check needs you too');
assert(text(card('f4')).includes('Sorting…')&&text(card('f1')).includes('Parser'),'each card names its area, or Sorting…');
assert.equal(card('f1').querySelector('.my-focus-chip').attrs['aria-label'],'Focus area: Parser','the card chip says what it names');
chips()[0].click();
assert.deepEqual(ids(),['f1','f2'],'an area filters My Work');
assert.equal(chips()[0].attrs['aria-pressed'],'true');
assert.equal(document.activeElement.dataset.myWork,`chip:${P}`,'the chip keeps the focus');
assert.equal(text(panel.querySelector('.my-work-heading')),'Parser','its name heads the requests');
chips()[0].click();
assert.deepEqual(ids(),['f1','f2','f3','f4'],'pressed again, all of My Work');
chips()[1].click();
assert(text(panel).includes('No requests in this area yet'),'an empty area says so');
chips()[1].click();
run(`mw.range='day';mw.rows.find(r=>r.id==='f2').updated=${now}-3*86400000;mw.rows.find(r=>r.id==='f1').updated=${now}-3*86400000;myWorkDraw()`);
chips()[0].click();
assert(!text(panel).includes('No requests in this area yet')&&text(panel).includes('No requests here yet'),'an area the range hides is not called empty');
chips()[0].click();
run(`mw.range='all';myWorkDraw()`);
card('f3').querySelector('.my-focus-chip').click();
assert.deepEqual(ids(),['f3'],'a card opens its area');
run(`mw.area='fa_00000000000f';myWorkDraw()`);
assert.equal(ids().length,4,'a choice no longer listed shows all');
// A suggested area a card opened stands with the chips while chosen (CodeRabbit on #755).
set([req('f1',{focus:P}),req('f5',{focus:L})]);
card('f5').querySelector('.my-focus-chip').click();
assert.deepEqual(chips().map(n=>[n.dataset.myWork,n.attrs['aria-pressed']]),[[`chip:${P}`,'false'],[`chip:${D}`,'false'],[`chip:${L}`,'true'],['chip:other','false'],['chip:suggested','false'],['chip:past-focus','false']],'a suggested area a card opened stands pressed');
chips()[2].click();
assert.deepEqual(ids(),['f1','f5'],'pressed again, all of My Work');
assert.equal(document.activeElement.className,'my-work-heading','the chip gone, the focus on the heading');
set([req('f1',{focus:P,status:'needs'}),req('f2',{focus:P}),req('f3',{focus:'other',status:'ready'}),req('f4')]);
// A chip with the focus holds a changed poll, as the tabs do (Codex on #755).
ctx.fetch=()=>Promise.resolve(response({requests:[req('f1',{focus:P,status:'needs',title:'Changed title'})],unlinked:0,areas}));
document.activeElement=chips()[0];await run('myWorkLive()');
assert(!text(panel).includes('Changed title')&&text(panel).includes('New work updates are ready.'),'held while a chip has the focus');
document.activeElement=null;run('mw.pending=null');
// Slice 10c-2a: the chosen area's header, Other's, a suggested and a done one's.
const withGoals=[{id:P,name:'Parser',status:'kept',goal:'Parse every file.',asked:3,latest:now-2*H,quotes:[{text:'parse it all',ts:now-2*H}],
    asks:[{id:'a1',title:'Speed it up',asked:2,latest:now-3*H,quotes:[{text:'faster please',ts:now-3*H}]}],page_word:null},
  {id:D,name:'The My Work docs',status:'suggested',goal:null,asked:null,latest:null,quotes:[],asks:[],page_word:'my work',evidence:'one request held several asks'},
  {id:L,name:'Lexer',status:'done',goal:null,asked:null,latest:null,quotes:[],asks:[],page_word:null}];
ctx.__rows={requests:[req('f1',{focus:P,status:'needs'}),req('f2',{focus:P,status:'shipped'}),req('f3',{focus:'other'})],unlinked:0,areas:withGoals,
  other_asks:[{id:'a9',title:'Tidy the README',asked:1,latest:now-H,quotes:[]}]};
run('Object.assign(mw,{rows:null,range:"day",area:null,saved:null});myWorkAccept(globalThis.__rows);myWorkDraw()');
assert.equal(panel.querySelectorAll('.my-focus-header').length,0,'no header for all of My Work');
chips()[0].click();
let head=text(panel.querySelector('.my-focus-header'));
for(const said of ['Parser','Parse every file.','Waiting on you','1 shipped','asked 3×','Not reviewed yet · 0 of 2 requests','Not started · 1','Speed it up','Asked 2×','Why it’s here · 1 quote'])
  assert(head.includes(said),`${said}: ${head}`);
assert(!head.includes('Suggested')&&!head.includes('reuses'),head);
run(`mw.rows.find(r=>r.id==='f2').updated=${now}-9*86400000;myWorkDraw()`);
assert(text(panel.querySelector('.my-focus-header')).includes('of 2 requests'),'all its requests, the range left aside');
run(`myFocusChoose('${D}')`);
head=text(panel.querySelector('.my-focus-header'));
for(const said of ['The My Work docs','Suggested','“The My Work docs” reuses the page name “my work”.','Nothing on it','0 shipped','No requests, so no progress to measure','Why it’s here: one request held several asks'])
  assert(head.includes(said),`${said}: ${head}`);
assert(!head.includes('asked')&&!head.includes('Not started'),head);
run(`myFocusChoose('${L}')`);
assert(text(panel.querySelector('.my-focus-header')).includes('Done'));
run("myFocusChoose('other')");
head=text(panel.querySelector('.my-focus-header'));
for(const said of ['Other','Requests that fit none of your focus areas.','0 shipped','Not started · 1','Tidy the README'])
  assert(head.includes(said),`${said}: ${head}`);
// A quotes disclosure stays open through a redraw, and with the focus in the header a changed poll waits.
run(`myFocusChoose('${P}')`);
const why=()=>panel.querySelector('.my-focus-header').querySelectorAll('details').find(n=>n.querySelector('summary').dataset.myWork===`why:area:${P}`);
why().open=true;for(const cb of why().listeners.toggle)cb();
run('myWorkDraw()');
assert.equal(why().open,true,'kept open');
ctx.fetch=()=>Promise.resolve(response({...ctx.__rows,requests:[req('f1',{focus:P,status:'needs',title:'Changed title'})]}));
document.activeElement=why().querySelector('summary');await run('myWorkLive()');
assert(!text(panel).includes('Changed title'),'held while the header has the focus');
document.activeElement=null;run('mw.pending=null');
ctx.ui.setLang('ja');run('myWorkDraw()');
assert(text(panel.querySelector('.my-focus-header')).includes('まだ始まっていない · 1')&&text(panel.querySelector('.my-focus-header')).includes('3 回頼みました、最後は'));
ctx.ui.setLang('en');
// Slice 10c-2b: the area's menu, its editors, a suggested area's Keep and Dismiss, each change's Undo.
{
const M='fa_000000000005',blank={asked:null,latest:null,quotes:[],asks:[],page_word:null};
const B=[{...blank,id:P,name:'Parser',status:'kept',source:'owner-goals',goal:'Parse every file.'},
  {...blank,id:D,name:'Docs',status:'kept',source:'owner',goal:null},
  {...blank,id:L,name:'Lexer',status:'suggested',source:'clustering',goal:null},
  {...blank,id:M,name:'The My Work notes',status:'suggested',source:'owner',goal:null,page_word:'my work'}];
const words=['my focus','my work','other','その他'];
const page={requests:[req('f1',{focus:P})],unlinked:0,areas:B,other_asks:[],page_words:words};
let posts=[],reply=()=>posted(200,{state:'applied'});
// The server as the page reads it again: a rename applied, a name of a rule's masked.
const applyRename=(body,r)=>{if(body.action==='rename'&&r.ok&&r.state!=='pending')B.find(a=>a.id===body.area).name=body.name.replace(/codename-[a-z]+/g,'[codename]');};
ctx.fetch=(url,options)=>{if(options?.method==='POST'){const body=JSON.parse(options.body);posts.push(body);const r=reply();return r.json().then(v=>{applyRename(body,{ok:r.ok,state:v.state});return r;});}return Promise.resolve(response(page));};
ctx.__rows=page;
run('Object.assign(mw,{rows:null,range:"all",area:null,saved:null,editing:null});myWorkAccept(globalThis.__rows);myWorkDraw()');
const hdr=()=>panel.querySelector('.my-focus-header');
const pick=key=>hdr().querySelectorAll('button').find(n=>n.dataset.myWork===key);
const status=()=>document.getElementById('status').textContent;
const undo=()=>panel.querySelectorAll('button').find(n=>n.dataset.myWork==='focus:undo');
const type=(field,value)=>{field.value=value;for(const cb of field.listeners.input)cb();};
run(`myFocusChoose('${P}')`);
const menu=hdr().querySelector('.my-focus-menu');
assert.equal(menu.querySelector('summary').attrs['aria-label'],'Change Parser');
assert.deepEqual([pick('menu:up').disabled,pick('menu:down').disabled],[true,false],'the first kept area moves only down');
// Move down sends the whole kept order; Undo sends the order before.
pick('menu:down').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'reorder',order:[D,P]});
assert.equal(status(),'Changed the order of your focus areas.');
assert.equal(document.activeElement.dataset.myWork,'focus:menu','the focus back on the menu');
undo().click();await ticks();
assert.deepEqual(posts.at(-1),{action:'reorder',order:[P,D]});
assert.equal(status(),'Undone: Changed the order of your focus areas.');
// Escape closes the menu, the focus on it.
const open=hdr().querySelector('.my-focus-menu');open.open=true;
document.activeElement=pick('menu:rename');
for(const cb of open.listeners.keydown)cb({key:'Escape'});
assert.equal(open.open,false);assert.equal(document.activeElement.dataset.myWork,'focus:menu');
// Rename: a page word holds Save and says so, a Japanese page name too; another name saves, Undo renames it back.
pick('menu:rename').click();
let field=hdr().querySelector('input');
assert.deepEqual([field.value,document.activeElement.dataset.myWork],['Parser','edit:rename']);
// Bounded in characters as the server counts them: 80 emoji fit, 81 do not, and it says why (CodeRabbit on #757).
type(field,'😀'.repeat(80));assert.equal(pick('edit:save').disabled,false);
type(field,'😀'.repeat(81));assert.equal(pick('edit:save').disabled,true);assert(text(hdr()).includes('At most 80 characters.'));
type(field,'My Work parser');
assert.equal(pick('edit:save').disabled,true);
assert(text(hdr()).includes('“My Work parser” reuses the page name “my work”. Pick a name that means only this area.'),text(hdr()));
type(field,'その他');assert.equal(pick('edit:save').disabled,true,'a Japanese page name');
type(field,'  ');assert.equal(pick('edit:save').disabled,true,'a blank name');
type(field,'Parsing');assert.equal(pick('edit:save').disabled,false);
assert(!text(hdr()).includes('reuses the page name “my work”. Pick'));
pick('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'rename',area:P,name:'Parsing'});
assert.equal(hdr().querySelectorAll('input').length,0,'saved, the editor closes');
assert.equal(status(),'Renamed “Parser” to “Parsing”.');
undo().click();await ticks();
assert.deepEqual(posts.at(-1),{action:'rename',area:P,name:'Parser'});
// The receipt names the new name as the gate kept it, never as it was typed (Codex on #757).
pick('menu:rename').click();type(hdr().querySelector('input'),'Parser codename-x');pick('edit:save').click();await ticks();
assert.equal(status(),'Renamed “Parser” to “Parser [codename]”.');
assert(!text(panel).includes('codename-x'),'nothing shows what was typed');
undo().click();await ticks();assert(!status().includes('codename-x'));
// A rename saved but not read yet names no new name.
reply=()=>posted(200,{state:'pending'});
pick('menu:rename').click();type(hdr().querySelector('input'),'Parsers');pick('edit:save').click();await ticks();
assert.equal(status(),'Renamed “Parser”.');
reply=()=>posted(200,{state:'applied'});
// A refusal keeps the editor and its draft, and says why.
reply=()=>posted(422,{code:'focus_page_word'});
pick('menu:rename').click();field=hdr().querySelector('input');type(field,'Parsers');
pick('edit:save').click();await ticks();
assert.equal(status(),'That name reuses a page name. Pick a name that means only this area.');
assert.equal(hdr().querySelector('input').value,'Parsers','the draft kept');
assert.equal(document.activeElement.dataset.myWork,'edit:rename','the focus back on the editor');
pick('edit:cancel').click();
assert.equal(hdr().querySelectorAll('input').length,0);assert.equal(document.activeElement.dataset.myWork,'focus:menu');
reply=()=>posted(200,{state:'applied'});
// A goal written where there was none has no Undo; the prompt names Edit goal.
run(`myFocusChoose('${D}')`);
assert(text(hdr()).includes('Write one sentence saying what this area is for (Edit goal).'));
pick('menu:edit-goal').click();field=hdr().querySelector('input');
assert.deepEqual([field.value,pick('edit:save').disabled],['',true]);
type(field,'Docs for the users.');pick('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'edit-goal',area:D,goal:'Docs for the users.'});
assert(text(panel.querySelector('.my-focus-change')).includes('This one can’t be undone from here.'));
// Merge a kept area: the choice moves to the area merged into; Undo puts it back in two steps.
run(`myFocusChoose('${P}')`);pick('menu:merge').click();
const into=hdr().querySelector('select');
assert.deepEqual(into.options.map(o=>[o.value,text(o)]),[['','Choose an area'],[D,'Docs'],[L,'Lexer · suggested'],[M,'The My Work notes · suggested']]);
assert.equal(pick('edit:save').disabled,true,'no area chosen');
into.value=D;for(const cb of into.listeners.change)cb();
pick('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'merge',area:P,into:D});
assert.equal(text(hdr().querySelector('h3')),'Docs','the choice on the area merged into');
const before=posts.length;undo().click();await ticks();
assert.deepEqual(posts.slice(before),[{action:'restore-suggestion',area:P},{action:'keep',area:P}]);
// A first step saved but not read yet holds the second back; the same Undo sends it (Codex on #757).
run(`myFocusChoose('${P}')`);pick('menu:merge').click();
let sel=hdr().querySelector('select');sel.value=D;for(const cb of sel.listeners.change)cb();
pick('edit:save').click();await ticks();
reply=()=>posted(200,{state:'pending'});
let at0=posts.length;undo().click();await ticks();
assert.deepEqual(posts.slice(at0),[{action:'restore-suggestion',area:P}]);
assert.equal(status(),'Saved the first step. Press Undo again once My Work has read it.');
reply=()=>posted(200,{state:'applied'});
at0=posts.length;undo().click();await ticks();
assert.deepEqual(posts.slice(at0),[{action:'keep',area:P}]);
assert.equal(panel.querySelectorAll('.my-focus-change').length,0,'then it is undone');
// A choice made while a merge saves stays (Codex on #757).
let free;reply=()=>posted(200,{state:'applied'});
run(`myFocusChoose('${P}')`);pick('menu:merge').click();
sel=hdr().querySelector('select');sel.value=D;for(const cb of sel.listeners.change)cb();
const held=ctx.fetch;ctx.fetch=(url,options)=>options?.method==='POST'?new Promise(r=>{free=()=>r(posted(200,{state:'applied'}));}):held(url,options);
pick('edit:save').click();await ticks();
run(`myFocusChoose('${L}')`);free();await ticks();ctx.fetch=held;
assert.equal(text(hdr().querySelector('h3')),'Lexer','the newer choice kept');
// Undo from Other, which has no menu, puts the focus on the heading.
run("myFocusChoose('other')");document.activeElement=undo();undo().click();await ticks();
assert.equal(document.activeElement,document.getElementById('heading'));
// A suggested area: Keep and Dismiss; a suggestion's Undo puts it back in Suggested, none for one you wrote.
run(`myFocusChoose('${L}')`);
assert(text(hdr()).includes('Suggested'));
pick('decide:keep').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'keep',area:L});
undo().click();await ticks();
assert.deepEqual(posts.at(-1),{action:'restore-suggestion',area:L});
pick('decide:dismiss').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'dismiss',area:L});assert.equal(status(),'Dismissed “Lexer”.');
run(`myFocusChoose('${M}')`);pick('decide:keep').click();await ticks();
assert(text(panel.querySelector('.my-focus-change')).includes('This one can’t be undone from here.'),'no Undo for a suggestion you wrote');
// Renamed away from a page word, it has no Undo back to it (Codex and CodeRabbit on #757).
pick('menu:rename').click();type(hdr().querySelector('input'),'Notes');pick('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'rename',area:M,name:'Notes'});
assert(text(panel.querySelector('.my-focus-change')).includes('This one can’t be undone from here.'),'no Undo back to a page word');
// Done, then its Undo gives the status back; a done area's menu offers Back in focus.
run(`myFocusChoose('${P}')`);pick('menu:done').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'done',area:P});assert.equal(status(),'Marked “Parser” done.');
undo().click();await ticks();assert.deepEqual(posts.at(-1),{action:'keep',area:P});
page.areas=B.map(a=>a.id===P?{...a,status:'done'}:a);run('myWorkAccept(globalThis.__rows);myWorkDraw()');
run(`myFocusChoose('${P}')`);
assert(pick('menu:keep')&&!pick('menu:done')&&!pick('menu:up'),'Back in focus, and no place in the kept order');
ctx.ui.setLang('ja');run('myWorkDraw()');
assert(text(hdr()).includes('フォーカスに戻す'));
ctx.ui.setLang('en');
}
// Slice 10c-3a: the Suggested page and Past focus.
{
const blank={asked:null,latest:null,quotes:[],asks:[],page_word:null,goal:null};
const qs=[0,1,2].map(i=>({text:`quote ${i}`,ts:now-(i+1)*H}));
const S=[{...blank,id:P,name:'Parser',status:'kept',source:'owner-goals'},
  {...blank,id:D,name:'Docs',status:'done',source:'owner',goal:'Docs for the users.'},
  {...blank,id:L,name:'Lexer',status:'suggested',source:'clustering',quotes:qs,asks:[{id:'a1',title:'Speed',asked:1,latest:now,quotes:[]}]},
  {...blank,id:'fa_000000000006',name:'Mine',status:'suggested',source:'owner'}];
const page={requests:[req('f1',{focus:L}),req('f2',{focus:P})],unlinked:0,areas:S,other_asks:[],page_words:[]};
const goals={current:{written:now,items:[]},window:{days:31,prompts:20},month:{month:'2026-10',prompts:12},
  months:[{month:'2026-09',summary:'Shipped the parser.',prompts:40,priorities:[{title:'Parse it all',product:'',asked:3,quote:{text:'parse it',ts:now-40*86400000}}]}]};
let posts=[],asked=0,reply=()=>posted(200,{state:'applied'});
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){posts.push(JSON.parse(options.body));return Promise.resolve(reply());}
  if(url.startsWith('/api/my-goals')){asked++;return Promise.resolve(response(goals));}
  return Promise.resolve(response(page));
};
ctx.__rows=page;
run('Object.assign(mw,{rows:null,range:"all",area:null,saved:null,editing:null,past:null});mw.decided.clear();myWorkAccept(globalThis.__rows);myWorkDraw()');
const pg=()=>panel.querySelector('.my-focus-page');
const btn=key=>panel.querySelectorAll('button').find(n=>n.dataset.myWork===key);
const cardOf=name=>pg().querySelectorAll('li').find(n=>n.className.includes('my-focus-card')&&text(n).startsWith(name));
assert.equal(text(chips().find(n=>n.dataset.myWork==='chip:suggested')),'2 suggested');
btn('chip:suggested').click();
assert.equal(btn('chip:suggested').attrs['aria-pressed'],'true');
assert.equal(panel.querySelectorAll('.my-work-grid').length,0,'a page of its own, in place of the requests');
let said=text(pg());
for(const w of ['Suggested focus areas','Waiting on you','0 of 2 decided','1 request placed in it · 1 ask not started','No requests placed in it yet',
  'Why it’s suggested: several Other requests read as steps of it.','quote 0','2 more quotes','Why it’s suggested: you wrote it.'])
  assert(said.includes(w),`${w}: ${said}`);
// Keep: the card says it with its own Undo, not the change bar; the decision stays until the page is left.
btn(`card:keep:${L}`).click();await ticks();
assert.deepEqual(posts.at(-1),{action:'keep',area:L});
assert(text(cardOf('Lexer')).includes('Kept. It’s in My focus now.')&&btn(`card:show:${L}`)&&btn(`card:undo:${L}`));
assert.equal(panel.querySelectorAll('.my-focus-change').length,0,'not in the change bar');
assert.equal(document.activeElement.dataset.myWork,`card:undo:${L}`);
assert(text(pg()).includes('1 of 2 decided'));
btn(`card:undo:${L}`).click();await ticks();
assert.deepEqual(posts.at(-1),{action:'restore-suggestion',area:L});
assert(btn(`card:keep:${L}`),'undecided again');
assert.equal(document.getElementById('status').textContent,'Put “Lexer” back in Suggested.');
// Codex on #776 (suggestion_save_reenables_the_area_opened_while_waiting): a kept area opened while
// a Keep saves; saved, its menu is usable, though the Keep's caller draws only on Suggested.
{
  let release=null;
  const kept=reply;
  reply=()=>new Promise(r=>{release=()=>r(posted(200,{state:'applied'}));});
  btn(`card:keep:${L}`).click();
  run(`mw.area="${P}";myWorkDraw()`);
  assert(btn('menu:rename')?.disabled,'its menu waits while the Keep saves');
  release();await ticks();
  reply=kept;
  assert(btn('menu:rename')&&!btn('menu:rename').disabled,'its menu is usable once it is saved');
  run('mw.area="suggested";mw.decided.clear();mw.saved=null;myWorkDraw()');
}
// #777 (a_save_that_ends_while_the_owner_types_in_search_keeps_the_field): the owner goes to the
// requests while a Keep saves and types in the search field; saved, the page drawn again, the field
// keeps its focus, its text and its caret.
{
  let release=null;
  const kept=reply;
  reply=()=>new Promise(r=>{release=()=>r(posted(200,{state:'applied'}));});
  btn(`card:keep:${L}`).click();
  run('mw.area=null;myWorkDraw()');
  const field=()=>panel.querySelectorAll('input').find(n=>n.dataset.myWork==='search');
  field().focus();field().value='lex';for(const cb of field().listeners.input)cb();
  Object.assign(field(),{selectionStart:2,selectionEnd:2,selectionDirection:'none'});
  release();await ticks();
  reply=kept;
  const typing=document.activeElement;
  assert(panel.contains(typing)&&typing===field(),'the field has the focus');
  assert.deepEqual([typing.value,typing.selectionStart,typing.selectionEnd],['lex',2,2],'its text and its caret');
  document.activeElement=null;
  run('mw.query="";mw.area="suggested";mw.decided.clear();mw.saved=null;myWorkDraw()');
}
// Saved but not read yet: the card stays undecided and says so (Codex on #758).
reply=()=>posted(200,{state:'pending'});
btn(`card:keep:${L}`).click();await ticks();
assert(btn(`card:keep:${L}`)&&!btn(`card:undo:${L}`),'not decided yet');
assert.equal(document.getElementById('status').textContent,'Saved. It shows here once My Work has read it.');
// Refused: the focus on the action again; an Undo refused, on the Undo (CodeRabbit on #758).
reply=()=>posted(409,{code:'focus_merged'});
document.activeElement=null;btn(`card:keep:${L}`).click();await ticks();
assert.equal(document.activeElement?.dataset.myWork,`card:keep:${L}`);
reply=()=>posted(200,{state:'applied'});
btn(`card:keep:${L}`).click();await ticks();
reply=()=>posted(409,{code:'focus_merged'});
document.activeElement=null;btn(`card:undo:${L}`).click();await ticks();
assert.equal(document.activeElement?.dataset.myWork,`card:undo:${L}`);
reply=()=>posted(200,{state:'applied'});
btn(`card:undo:${L}`).click();await ticks();
// Codex on #776 (applied_keep_stays_decided_when_its_read_fails): applied, the Keep is decided
// though the read after it fails, and its Undo is sent.
{
  const kept=ctx.fetch;
  ctx.fetch=(url,options)=>options?.method==='POST'?kept(url,options):Promise.resolve({ok:false,status:503,json:async()=>({})});
  btn(`card:keep:${L}`).click();await ticks();
  assert(btn(`card:show:${L}`)&&btn(`card:undo:${L}`),'decided, with Show it and its Undo');
  assert.equal(document.getElementById('status').textContent,'Saved. The page shows it at its next read.');
  ctx.fetch=kept;
  btn(`card:undo:${L}`).click();await ticks();
  assert.deepEqual(posts.at(-1),{action:'restore-suggestion',area:L});
  assert(btn(`card:keep:${L}`),'undecided again');
}
// Dismiss an area you wrote: no Undo, as the server never puts it back in Suggested (Codex and
// CodeRabbit on #758).
btn('card:dismiss:fa_000000000006').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'dismiss',area:'fa_000000000006'});
assert(text(cardOf('Mine')).includes('Dismissed. It won’t be suggested again'));
assert(!btn('card:show:fa_000000000006')&&!btn('card:undo:fa_000000000006'),'nothing to show, no Undo');
assert.equal(document.activeElement,document.getElementById('heading'));
// Merge from the card: it reads Merged, the page stays; Show it opens the area merged into.
btn(`card:merge:${L}`).click();
const into=pg().querySelector('select');into.value=P;for(const cb of into.listeners.change)cb();
btn('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'merge',area:L,into:P});
assert(text(cardOf('Lexer')).includes('Merged into Parser. Its requests show there now.'));
assert.equal(run('mw.area'),'suggested','the page stays');
// A rename leaves the card undecided and goes to the change bar.
run('mw.decided.delete("'+L+'");myWorkDraw()');
btn(`card:rename:${L}`).click();
const name=pg().querySelector('input');name.value='Lexing';for(const cb of name.listeners.input)cb();
btn('edit:save').click();await ticks();
assert.deepEqual(posts.at(-1),{action:'rename',area:L,name:'Lexing'});
assert(panel.querySelector('.my-focus-change')&&btn(`card:keep:${L}`),'the change bar, the card undecided');
// Leaving the page ends the visit's decisions.
btn('chip:suggested').click();btn('chip:suggested').click();
assert(btn('card:keep:fa_000000000006'),'Mine undecided again');
// A Keep saving as the page is left records nothing in the next visit (#759).
let free;const live=ctx.fetch;
ctx.fetch=(url,options)=>options?.method==='POST'?new Promise(r=>{free=()=>r(posted(200,{state:'applied'}));}):live(url,options);
btn('card:keep:fa_000000000006').click();await ticks();
btn('chip:suggested').click();free();await ticks();ctx.fetch=live;
btn('chip:suggested').click();
assert(btn('card:keep:fa_000000000006'),'the next visit shows it as the server has it, undecided here');
// With focus in the page, a changed poll waits.
document.activeElement=btn(`card:keep:${L}`);
ctx.__live=ctx.fetch;ctx.fetch=(url,options)=>url.startsWith('/api/my-work')?Promise.resolve(response({...page,requests:[req('f1',{focus:L,title:'Changed title'})]})):ctx.__live(url,options);
await run('myWorkLive()');ctx.fetch=ctx.__live;
assert(run('mw.pending')&&!run('mw.rows.some(r=>r.title==="Changed title")'),'held');
assert(text(panel).includes('New work updates are ready.')&&panel.querySelectorAll('button').some(n=>text(n)==='Show updates'),'and its banner shown on the page (Codex and CodeRabbit on #758)');
document.activeElement=null;run('mw.pending=null');
// Show it opens the area merged into.
btn(`card:merge:${L}`).click();const into2=pg().querySelector('select');into2.value=P;for(const cb of into2.listeners.change)cb();
btn('edit:save').click();await ticks();
btn(`card:show:${L}`).click();
assert.equal(text(panel.querySelector('.my-focus-header').querySelector('h3')),'Parser');
// Past focus: the done areas, each opening on its requests; month by month, read when it opens.
btn('chip:past-focus').click();await ticks();
assert.equal(asked,1,'My Goals\' months read when it opens');
said=text(pg());
for(const w of ['Past focus','Areas you finished','Done areas','Docs','Docs for the users.','Month by month','Current','Parser · first in your focus today','12 prompts',
  'Archived','Parse it all','40 prompts'])
  assert(said.includes(w),`${w}: ${said}`);
const month=pg().querySelectorAll('details').find(n=>n.querySelector('summary').dataset.myWork==='month:2026-09');
assert(text(month).includes('Shipped the parser.')&&text(month).includes('asked 3×')&&text(month).includes('No request reviews cover this month'));
btn(`done:${D}`).click();
assert.equal(text(panel.querySelector('.my-focus-header').querySelector('h3')),'Docs','a done area opens on its requests');
// Nothing to show: no done area and no months.
page.areas=S.filter(a=>a.status!=='done');goals.current=null;goals.months=[];
run('myWorkAccept(globalThis.__rows);mw.past=null;myWorkDraw()');
btn('chip:past-focus').click();await ticks();
assert(text(pg()).includes('Nothing here yet'),text(pg()));
ctx.ui.setLang('ja');run('myWorkDraw()');
assert(text(pg()).includes('過去のフォーカス'));
ctx.ui.setLang('en');
run('mw.area=null;mw.decided.clear()');
}
set([req('f1',{focus:P,status:'needs'}),req('f2',{focus:P}),req('f3',{focus:'other',status:'ready'}),req('f4')]);
// Move to… on a request's page: the areas that take requests but its own, then Other.
run('mw.area=null;myWorkDraw()');
let moves=[],at=P;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){const body=JSON.parse(options.body);moves.push([url,body]);at=body.area;return Promise.resolve(posted(200,{state:'applied'}));}
  return Promise.resolve(response({requests:[req('f1',{focus:at,status:'needs'})],unlinked:0,areas}));
};
card('f1').querySelector('button').click();
const move=()=>panel.querySelector('.my-focus-move');
const pick=to=>move().querySelectorAll('button').find(n=>n.dataset.myWork===`move:${to}`).click();
assert.equal(move().tagName.toLowerCase(),'details','a disclosure, not a select its arrow keys would commit');
assert.equal(text(move().querySelector('summary')),'Move to…');
assert.deepEqual(move().querySelectorAll('button').map(n=>[n.dataset.myWork,text(n)]),[[`move:${D}`,'Docs'],[`move:${L}`,'Lexer · suggested'],['move:other','Other']]);
move().querySelector('summary').click();await ticks();
assert.equal(moves.length,0,'opening it moves nothing');
pick(D);await ticks();
assert.deepEqual(moves,[['/api/my-focus',{action:'move-request',request:'f1',area:D}]],'the move names the request and the area');
let bar=text(panel.querySelector('.my-focus-change'));
assert(bar.includes('Moved “Request f1” to Docs.')&&bar.includes('Saved on this computer.')&&bar.includes('Undo'),bar);
assert.equal(document.getElementById('status').textContent,'Moved “Request f1” to Docs.','the page live region reads it out');
assert.equal(panel.querySelector('.my-focus-change').attrs['aria-live'],undefined,'the bar, drawn anew, is not a live region');
assert.equal(document.activeElement.dataset.myWork,'focus:move','the focus is back on Move to…');
panel.querySelectorAll('button').find(n=>n.dataset.myWork==='focus:undo').click();await ticks();
assert.deepEqual(moves[1],['/api/my-focus',{action:'move-request',request:'f1',area:P}],'Undo moves it back');
assert.equal(panel.querySelectorAll('.my-focus-change').length,0,'undone, the bar is gone');
assert.equal(document.getElementById('status').textContent,'Moved “Request f1” to Parser.');
assert.equal(document.activeElement.dataset.myWork,'focus:move');
// #775: the read after a move fails: what was saved is said so, and Move to… is usable again.
{
  const kept=ctx.fetch;
  ctx.fetch=(url,options)=>options?.method==='POST'?Promise.resolve(posted(200,{state:'applied'})):Promise.resolve({ok:false,status:503,json:async()=>({})});
  pick(D);await ticks();
  assert.equal(document.getElementById('status').textContent,'Saved. The page shows it at its next read.');
  assert(move()&&move().querySelectorAll('button').every(n=>!n.disabled),'Move to… is usable again');
  // Codex on #776 (applied_focus_move_keeps_undo_after_a_failed_reread_recovers): it was applied,
  // so it keeps its Undo, and a read that works later leaves it.
  assert(text(panel.querySelector('.my-focus-change')).includes('Undo'),'the move keeps its Undo');
  ctx.fetch=kept;
  await run('myWorkReread()');run('myWorkDraw()');
  assert(text(panel.querySelector('.my-focus-change')).includes('Undo'),'and after a read that works');
  panel.querySelectorAll('button').find(n=>n.dataset.myWork==='focus:undo').click();await ticks();
}
// #777 (a_second_focus_move_after_a_failed_reread_undoes_to_the_applied_area): the reads after
// two moves fail; the second's Undo takes the request back where the first put it, as the store
// has it.
{
  const kept=ctx.fetch;
  ctx.fetch=(url,options)=>options?.method==='POST'?kept(url,options):Promise.resolve({ok:false,status:503,json:async()=>({})});
  pick(D);await ticks();
  pick(L);await ticks();
  panel.querySelectorAll('button').find(n=>n.dataset.myWork==='focus:undo').click();await ticks();
  assert.deepEqual(moves.at(-1),['/api/my-focus',{action:'move-request',request:'f1',area:D}],'back to Docs, where the first move put it');
  ctx.fetch=kept;
  await run('myWorkReread()');run('myWorkDraw()');
}
// From Sorting… there is nothing to move it back to.
at=null;set([req('f1',{status:'needs'})]);
card('f1').querySelector('button').click();
pick('other');await ticks();
assert(text(panel.querySelector('.my-focus-change')).includes('This one can’t be undone from here.'));
// Nor back into a done area, which takes no requests (Codex on #755).
const O='fa_000000000004',withDone=[...areas,{id:O,name:'Old',status:'done'}];
ctx.fetch=(url,options)=>options?.method==='POST'?Promise.resolve(posted(200,{state:'applied'})):Promise.resolve(response({requests:[req('f1',{focus:P})],unlinked:0,areas:withDone}));
set([req('f1',{focus:O})],withDone);
card('f1').querySelector('button').click();
pick(P);await ticks();
assert(text(panel.querySelector('.my-focus-change')).includes('This one can’t be undone from here.'),'no Undo into a done area');
// A refusal's words.
ctx.fetch=(url,options)=>options?.method==='POST'?Promise.resolve(posted(409,{code:'focus_move_target'})):Promise.resolve(response({requests:[req('f1',{focus:'other'})],unlinked:0,areas}));
pick(D);await ticks();
assert.equal(document.getElementById('status').textContent,'That area takes no requests now.');
// A page opened while a move saves is not drawn over (Codex on #755).
let release;
ctx.fetch=(url,options)=>options?.method==='POST'?new Promise(r=>{release=()=>r(posted(200,{state:'applied'}));}):Promise.resolve(response({requests:[req('f1',{focus:P})],unlinked:0,areas}));
pick(P);await ticks();
run("view='settings';document.getElementById('panel').replaceChildren(document.createElement('section'))");
release();await ticks();
assert.equal(panel.querySelectorAll('.my-focus-move').length,0,'the page opened stays');
run("view='my_work'");
// Japanese.
ctx.ui.setLang('ja');set([req('f1',{focus:P,status:'needs'}),req('f4')]);
assert(text(card('f4')).includes('振り分け中…')&&chips().some(n=>text(n).startsWith('その他')),'the chips and the card in Japanese');
assert.equal(panel.querySelector('.my-focus-chips').attrs['aria-label'],'マイフォーカス');
ctx.ui.setLang('en');
console.log('PASS: My Focus: chips with counts, the area filter, the card chip and Sorting…, the area and Other headers, the menu, editors, Keep and Dismiss with each Undo, the Suggested page and Past focus, Move to… with its bar and Undo, a refusal, JA/EN');
}

// Parity row X9: a long prompt is clamped behind Show more, as claude-mem's PromptCard does; a short one has no button.
{
const entry=vm.runInContext('feedEntry',ctx);
const prompt=text=>entry({kind:'prompt',id:'device:1',ts:0,agent:'claude',repo_name:'r',repo:'r',text});
const buttons=node=>node.querySelectorAll('button').filter(b=>b.className==='show-more');
for(const long of ['x'.repeat(601),Array.from({length:9},(_,i)=>`line ${i}`).join('\n')]){
  const [more]=buttons(prompt(long));
  assert.ok(more,'a long prompt has a Show more button');
  assert.equal(more.attrs['aria-expanded'],'false');
  assert.equal(more.dataset.i18n,'show_more');
  more.click();
  assert.equal(more.attrs['aria-expanded'],'true','Show more opens it');
  assert.equal(more.dataset.i18n,'show_less');
  more.click();
  assert.equal(more.attrs['aria-expanded'],'false','Show less closes it');
}
for(const short of ['x'.repeat(600),Array.from({length:8},(_,i)=>`line ${i}`).join('\n')]){
  assert.equal(buttons(prompt(short)).length,0,'a prompt within 8 lines and 600 characters has no button');
}
}

// Parity row N3: a note an agent saved is a Feed card labelled Note, as claude-mem's feed shows a manual memory.
{
const entry=vm.runInContext('feedEntry',ctx);
const note=()=>text(entry({kind:'note',id:'oboete-note:0123456789abcdef',ts:0,agent:null,repo_name:'r',repo:'r',
  title:'Release tagging',subtitle:'',narrative:'Releases are tagged from main.',facts:[],concepts:[],files_read:[],files_modified:[]}));
const shown=note();
for(const part of ['Note','Release tagging','Saved by an agent','narrative','Releases are tagged from main.','oboete-note:0123456789abcdef'])
  assert.ok(shown.includes(part),`a note's card shows ${part}: ${shown}`);
ctx.ui.setLang('ja');
const ja=note();
assert.ok(ja.includes('メモ')&&ja.includes('エージェントが保存したメモ'),ja);
ctx.ui.setLang('en');
console.log('PASS: a saved note is a Feed card labelled Note, in both languages');
}

// Parity row 16 (docs/page.md slice 2): the Sessions tab lists the sessions; a session's page shows
// its items, a chip per kind with its count, filters by them, and Back returns to the list with the
// opened card focused; in both languages.
{
const run=code=>vm.runInContext(code,ctx);
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const response=value=>({ok:true,status:200,json:async()=>value});
const settle=async()=>{for(let i=0;i<5;i++)await new Promise(resolve=>setTimeout(resolve,0));};
const asked=[];
const sessions=[
  {key:'a1b2c3d4e5f60718',agent:'codex',session:'0b5e7c2d-later',repo:'example.test/team/moss',repo_name:'moss',title:'Fix the build.',started:2000,items:1},
  {key:'0f1e2d3c4b5a6978',agent:'claude',session:'7f3a91c4-earlier',repo:'example.test/team/fern',repo_name:'fern',title:null,started:1000,items:3}];
const items=[
  {kind:'card',id:'5.0',ts:30,type:'feature',agent:'claude',repo_name:'fern',repo:'example.test/team/fern',title:'Parser built',subtitle:'',narrative:'Built.',facts:[],concepts:[],files_read:[],files_modified:[]},
  {kind:'summary',id:'S6',ts:20,agent:'claude',repo_name:'fern',repo:'example.test/team/fern',fields:{request:'Build the parser.'}},
  {kind:'prompt',id:'dev:1',ts:10,agent:'claude',repo_name:'fern',repo:'example.test/team/fern',text:'Build the parser.'},
  {kind:'card',id:'4.0',ts:5,type:'curiosity',agent:'claude',repo_name:'fern',repo:'example.test/team/fern',title:'Odd one',subtitle:'',narrative:'Odd.',facts:[],concepts:[],files_read:[],files_modified:[]}];
ctx.fetch=url=>{
  asked.push(url);
  if(url.startsWith('/api/sessions?'))return Promise.resolve(response({sessions,next:null}));
  if(url.startsWith('/api/feed?'))return Promise.resolve(response({items,next:null}));
  if(url.startsWith('/api/repos?'))return Promise.resolve(response({current:'',repos:[]}));
  return Promise.resolve(response({}));
};
run('reposLoaded=true;setView("sessions")');
await run('show()');
const list=document.getElementById('list');
const cards=()=>list.querySelectorAll('button').filter(b=>b.className==='session-card');
assert.deepEqual(cards().map(b=>b.dataset.session),['a1b2c3d4e5f60718','0f1e2d3c4b5a6978'],'the sessions, newest first');
assert(text(cards()[0]).includes('Fix the build.')&&text(cards()[0]).includes('1 item')&&text(cards()[0]).includes('0b5e7c2d'),text(cards()[0]));
assert(text(cards()[1]).includes('fern')&&text(cards()[1]).includes('3 items'),'a session with no prompt left is titled by its repository');
assert.equal(text(document.getElementById('heading')),'Sessions');
cards()[1].click();
await settle();
assert(asked.some(u=>u.startsWith('/api/feed?')&&u.includes('session=0f1e2d3c4b5a6978')),'the page reads that session\'s items');
const panel=document.getElementById('panel');
const shown=text(panel);
for(const part of ['← Back to sessions','claude','7f3a91c4-earlier','Prompt (1)','Summary (1)','Feature (1)','Other (1)','4 of 4','Parser built'])
  assert(shown.includes(part),`the session's page shows ${part}: ${shown}`);
const entries=()=>panel.querySelectorAll('li').filter(n=>n.className==='feed-entry');
const chip=kind=>panel.querySelectorAll('button').find(b=>b.dataset.kind===kind);
chip('summary').focus();chip('summary').click();
assert.deepEqual(entries().map(n=>n.hidden),[true,false,true,true],'a chip shows its kind alone');
assert(document.activeElement?.dataset?.kind==='summary'&&panel.contains(document.activeElement),'the chip keeps the focus');
assert.equal(chip('summary').attrs['aria-pressed'],'true');
assert(text(panel).includes('1 of 4'),text(panel));
chip('prompt').click();
assert.deepEqual(entries().map(n=>n.hidden),[true,false,false,true],'two chips, two kinds');
chip('summary').click();chip('prompt').click();
assert.deepEqual(entries().map(n=>n.hidden),[false,false,false,false],'no chip on, every item');
panel.querySelectorAll('button').find(b=>b.className.includes('session-back')).click();
await settle();
assert.deepEqual(cards().map(b=>b.dataset.session),['a1b2c3d4e5f60718','0f1e2d3c4b5a6978'],'back to the list');
assert.equal(document.activeElement?.dataset?.session,'0f1e2d3c4b5a6978','the opened card has the focus again');
assert.equal(asked.filter(u=>u.startsWith('/api/sessions?')).length,1,'the list is not read again on the way back');
// A live read keeps the focused card's focus; a change seen while a session's page was open reads
// the list again on the way back (S6).
cards()[0].focus();
await run('refresh(true)');
assert.equal(asked.filter(u=>u.startsWith('/api/sessions?')).length,2,'a live read of the list');
assert(document.activeElement?.dataset?.session==='a1b2c3d4e5f60718'&&list.contains(document.activeElement),'the focused card keeps the focus');
cards()[1].click();
await settle();
await run('refresh(true)');
panel.querySelectorAll('button').find(b=>b.className.includes('session-back')).click();
await settle();
assert.equal(asked.filter(u=>u.startsWith('/api/sessions?')).length,3,'the list is read again after a change');
ctx.ui.setLang('ja');
run('setView("sessions")');
await run('show()');
assert(text(document.getElementById('heading')).includes('会話')&&text(cards()[1]).includes('3 件'),'the list in Japanese');
cards()[1].click();
await settle();
assert(text(panel).includes('← 会話の一覧へ')&&text(panel).includes('プロンプト (1)')&&text(panel).includes('4 件中 4 件'),text(panel));
ctx.ui.setLang('en');
run('setView("timeline")');
console.log('PASS: Sessions: the list, a session\'s page, its kind chips, Back with the focus, in both languages');
}

// Parity row 16, S6 (Codex and CodeRabbit on #635): a live read's first page takes the place of
// the sessions it covers: one no longer listed goes, a changed one moves to its place, and a first
// page that does not reach the list shown starts it again, More reading on from its cursor.
{
const run=code=>vm.runInContext(code,ctx);
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const response=value=>({ok:true,status:200,json:async()=>value});
const session=(key,started)=>({key,agent:'claude',session:`s-${key}`,repo:'example.test/team/fern',repo_name:'fern',title:`Title ${key}`,started,items:1});
let reply={sessions:[],next:null};
ctx.fetch=url=>{
  if(url.startsWith('/api/sessions?'))return Promise.resolve(response(reply));
  if(url.startsWith('/api/repos?'))return Promise.resolve(response({current:'',repos:[]}));
  return Promise.resolve(response({}));
};
const shown=()=>document.getElementById('list').querySelectorAll('button').filter(b=>b.className==='session-card').map(b=>b.dataset.session);
reply={sessions:[session('aaaaaaaaaaaaaaaa',3000),session('bbbbbbbbbbbbbbbb',2000),session('cccccccccccccccc',1000)],next:null};
run('reposLoaded=true;setView("sessions")');
await run('show()');
assert.deepEqual(shown(),['aaaaaaaaaaaaaaaa','bbbbbbbbbbbbbbbb','cccccccccccccccc']);
// b's first prompt removed: it starts later and moves up; c's prompts all removed: it goes.
reply={sessions:[session('bbbbbbbbbbbbbbbb',3500),session('aaaaaaaaaaaaaaaa',3000)],next:null};
await run('refresh(true)');
assert.deepEqual(shown(),['bbbbbbbbbbbbbbbb','aaaaaaaaaaaaaaaa'],'a changed session moves, a removed one goes');
assert(!text(document.getElementById('list')).includes('Title cccccccccccccccc'),'the removed session\'s title is gone');
// More than a page begun since: the first page does not reach the list, which starts again.
const fresh=Array.from({length:3},(_,i)=>session(`d${String(i).padStart(15,'0')}`,9000-i));
reply={sessions:fresh,next:'[8998,"dev",1]'};
await run('refresh(true)');
assert.deepEqual(shown(),fresh.map(s=>s.key),'the list starts again from the first page');
assert.equal(run('ses.next'),'[8998,"dev",1]','More reads on from the first page\'s cursor');
// A first page that reaches the list keeps the pages read after it.
reply={sessions:[session('eeeeeeeeeeeeeeee',9500),...fresh.slice(0,2)],next:'[8999,"dev",2]'};
await run('refresh(true)');
assert.deepEqual(shown(),['eeeeeeeeeeeeeeee',...fresh.map(s=>s.key)],'joined: the pages read stay');
assert.equal(run('ses.next'),'[8998,"dev",1]','and so does their cursor');
run('setView("timeline")');
console.log('PASS: Sessions live: removed sessions go, changed ones move, a list that does not join starts again');
}

// Parity row 3, M5 slice 5: Forget previews and confirms an item, then reads the stored view again.
{
const run=code=>vm.runInContext(code,ctx);
const settle=async()=>{for(let i=0;i<3;i++)await new Promise(resolve=>setImmediate(resolve));};
Object.defineProperty(Node.prototype,'isConnected',{get(){return [...ids.values()].some(root=>root.contains(this));}});
const json=(value,status=200)=>({ok:status===200,status,headers:{get:()=> 'application/json'},json:async()=>structuredClone(value)});
const prompt={kind:'prompt',id:'device:42',ts:10,agent:'claude',repo_name:'fern',repo:'example.test/team/fern',text:'Build the parser.'};
const note={kind:'note',id:'oboete-note:0123456789abcdef',ts:20,agent:null,repo_name:'fern',repo:'example.test/team/fern',
  title:'Keep it readable',subtitle:'',narrative:'Use the existing helpers.',facts:[],concepts:[],files_read:[],files_modified:[]};
const session={key:'a1b2c3d4e5f60718',agent:'claude',session:'full-session-id',repo:prompt.repo,repo_name:'fern',title:'Build the parser.',started:1,items:1};
const claimUid='0123456789abcdef'.repeat(4);
const claim={uid:claimUid,quotes:[],history:[],kind:'decision',status:'decided',label:'owner',when:0,delivered:false,
  speaker:'user',scope:'repository',repo:prompt.repo,text:'Keep it readable.',later:null,supersedes:[],ended_by:[],muted:false};
const preview={preview_key:'a'.repeat(64),records:7,copies:2,claims:3,cards:4,summaries:0,written:1,again:0};
const started={started:true,records:7,logged:true,cleaning:true};
const previewLines=block=>block.querySelectorAll('p').map(text);
const limits={
  en:'What was already given to an agent or sent to a curator is not taken back, and a conversation that starts during the cleaning may get only part of its memory.',
  ja:'すでにエージェントに渡した内容や要約役に送った内容は取り消せません。掃除の間に始まった会話には、記憶の一部しか渡らないことがあります。',
};
const requests=[];
let holdFeed=false,items=[prompt],sessions=[session],sessionNext=null,olderSessions=[];
ctx.fetch=(url,options={})=>{
  const request={url,options,body:options.body?JSON.parse(options.body):null};requests.push(request);
  if(options.method==='POST'||url.startsWith('/api/timeline?')||holdFeed&&url.startsWith('/api/feed?'))
    return new Promise((resolve,reject)=>Object.assign(request,{resolve,reject}));
  if(url.startsWith('/api/feed?'))return Promise.resolve(json({items,next:null}));
  if(url.startsWith('/api/sessions?')){
    const older=new URL(url,'https://oboete.test').searchParams.has('before');
    return Promise.resolve(json({sessions:older?olderSessions:sessions,next:older?null:sessionNext}));
  }
  if(url.startsWith('/api/claim?'))return Promise.resolve(json(claim));
  if(url.startsWith('/api/repos?'))return Promise.resolve(json({current:'',repos:[]}));
  throw new Error(`unexpected read: ${url}`);
};
ctx.ui.setLang('en');run('reposLoaded=true;setView("timeline")');
await run('show()');
const list=document.getElementById('list');
let button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
assert(button,'a prompt has a Forget button');
button.focus();button.click();
assert(button.disabled,'Forget is disabled while the preview is read');
const request=requests.at(-1);
assert.equal(request.url,'/api/forget/preview');
assert.equal(request.options.method,'POST');
assert.equal(request.options.headers['X-Oboete-Token'],'0123456789abcdef0123456789abcdef');
assert.deepEqual(request.body,{item:{kind:'prompt',id:'device:42'}});
request.resolve(json(preview));await settle();
const block=list.querySelector('.forget-preview');
assert(block,'the preview is an in-page confirmation');
assert(text(block).includes('This forgets the prompt, and the 3 memories, 4 cards and 0 summaries made from it.'),text(block));
assert(text(block).includes('This cannot be undone.'),text(block));
const cancel=block.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_cancel');
assert.equal(document.activeElement,cancel,'Cancel receives focus');
const beforeCancel=requests.length;
cancel.click();
assert.equal(requests.length,beforeCancel,'Cancel sends nothing');
assert.equal(list.querySelector('.forget-preview'),null);
assert.equal(document.activeElement,button,'Cancel restores the opener focus');
assert(!button.disabled,'Cancel enables the opener');
for(const [language,lines] of [
  ['en',[
    'This forgets the prompt, and the 3 memories, 4 cards and 0 summaries made from it.',
    'It also forgets 2 records of the same conversation from another source, which may hold a copy of it.',
    'The 1 memories written with those cards lose their text until they are curated again.',
    "The agent's own transcript files keep their copy: delete it there too if it must go.",
    limits.en,'This cannot be undone.',
  ]],
  ['ja',[
    'このプロンプトと、そこから作った記憶 3 件・カード 4 件・要約 0 件を忘れます。',
    '同じ会話を別の記録元から取り込んだ記録 2 件も、写しを含むことがあるため一緒に忘れます。',
    'それらのカードと一緒に作られた記憶 1 件は、作り直されるまで文章が消えます。',
    'エージェント自身の会話ファイルには写しが残ります。必要ならそちらも消してください。',
    limits.ja,'この操作は取り消せません。',
  ]],
]){
  ctx.ui.setLang(language);
  for(const copies of [2,0]){
    button.click();requests.at(-1).resolve(json({...preview,copies,written:copies?1:0}));await settle();
    const shown=list.querySelector('.forget-preview');
    assert.deepEqual(previewLines(shown),copies?lines:[lines[0],...lines.slice(3)],
      `${language}: prompt disclosures have counts and order, and omit zero copies/written`);
    shown.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_cancel').click();
  }
}
ctx.ui.setLang('en');
button.click();requests.at(-1).resolve(json(preview));await settle();
let confirmation=list.querySelector('.forget-preview');
let confirm=confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm');
confirm.click();
let start=requests.at(-1);
assert.equal(start.url,'/api/forget/start');
assert.deepEqual(start.body,{item:{kind:'prompt',id:'device:42'},preview_key:'a'.repeat(64),confirmed:true});
assert(confirmation.querySelectorAll('button').every(b=>b.disabled),'both choices disable during start');
const duringStart=requests.length;
confirm.click();confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_cancel').click();
assert.equal(requests.length,duringStart,'a pending start cannot be repeated or cancelled');
assert(list.contains(confirmation),'Cancel while start is pending leaves the same confirmation block in the page');
assert(list.contains(button),'the prompt stays while start is pending');
holdFeed=true;start.resolve(json(started));await settle();
const reread=requests.at(-1);
assert(reread.url.startsWith('/api/feed?')&&!reread.options.method,'success reads the feed again');
// The next server read alone determines what remains on the page.
reread.resolve(json({items:[prompt],next:null}));holdFeed=false;await settle();
assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.');
// The store's change brings the live view's redraw, which clears the line: the result stays.
run('setStatus("")');
assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.',
  'a redraw right after a forget keeps its result in the status line');
run('setStatus("Another message")');
assert.equal(document.getElementById('status').textContent,'Another message','another message replaces it');
run('setStatus("")');
assert.equal(document.getElementById('status').textContent,'','and the next redraw clears that one');
assert(text(list).includes('Build the parser.'),'an item still returned by the server stays visible');
assert.equal(list.querySelector('.forget-preview'),null);
button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
button.click();requests.at(-1).resolve(json({...preview,preview_key:'b'.repeat(64)}));await settle();
confirmation=list.querySelector('.forget-preview');
confirm=confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm');
confirm.click();start=requests.at(-1);
assert.equal(start.body.preview_key,'b'.repeat(64),'a new preview supplies the next key');
const beforeStale=requests.length;
start.resolve(json({code:'forget_stale',field:'preview_key'},409));await settle();
assert.equal(document.getElementById('status').textContent,'It changed since the preview. Press Forget again to see it.');
assert.equal(requests.length,beforeStale,'stale does not preview, retry or reload on its own');
assert.equal(list.querySelector('.forget-preview'),null);
assert.equal(document.activeElement,button,'a refused start restores focus');
button.click();requests.at(-1).resolve(json(preview));await settle();
confirmation=list.querySelector('.forget-preview');
const beforeEscape=requests.length;
let escaped=false;
for(const cb of confirmation.listeners.keydown||[])cb({key:'Escape',preventDefault(){escaped=true;}});
assert(escaped,'Escape closes the confirmation');
assert.equal(list.querySelector('.forget-preview'),null);
assert.equal(requests.length,beforeEscape,'Escape sends nothing');
assert.equal(document.activeElement,button,'Escape restores focus');
for(const [language,done,waiting,later,notLogged] of [
  ['en','Forgotten. The stores are cleaned in the background.',
    'Forgotten. Its cleaning starts after the one already running.',
    'Forgotten. Its cleaning could not be started now: the worker starts it at its next look, or run oboete forget --continue.',
    'The request could not be written to every log: raw.db holds it, but a restore from the backups alone would bring the text back.'],
  ['ja','忘れました。保存先の掃除は裏で続きます。',
    '忘れました。保存先の掃除は、いま動いている掃除が終わってから始まります。',
    '忘れました。保存先の掃除は今は始められませんでした。ワーカーが次に見回ったときに始めます(すぐ始めるには oboete forget --continue を実行してください)。',
    '忘れる依頼をすべての控えには書けませんでした。raw.db には残っていますが、バックアップだけから戻すと、忘れた内容が戻ります。'],
]){
  ctx.ui.setLang(language);
  for(const [cleaning,after,said] of [[true,false,done],[true,true,waiting],[false,false,later]])for(const logged of [true,false]){
    button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
    button.click();requests.at(-1).resolve(json(preview));await settle();
    list.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
    requests.at(-1).resolve(json({...started,cleaning,after,logged}));await settle();
    assert.equal(document.getElementById('status').textContent,
      said+(logged?'':` ${notLogged}`),
      `${language}: when the cleaning starts and whether it was logged are said together in the status line`);
  }
}
ctx.ui.setLang('en');

items=[note,{...note,kind:'card',id:'7.0'},
  {kind:'summary',id:'S8',ts:5,agent:'claude',repo_name:'fern',repo:prompt.repo,fields:{request:'Build the parser.'}}];
ctx.ui.setLang('ja');await run('show()');
const entries=list.querySelectorAll('li').filter(n=>n.className==='feed-entry');
const noteButton=entries[0].querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
assert(noteButton,'a note has a Forget button');
assert.equal(text(noteButton),'忘れる');
for(const entry of entries.slice(1))assert(!entry.querySelectorAll('button').some(b=>b.dataset.i18n==='forget_button'),
  'cards and summaries have no Forget button');
noteButton.click();
assert.deepEqual(requests.at(-1).body,{item:{kind:'note',id:'oboete-note:0123456789abcdef'}});
requests.at(-1).resolve(json(preview));await settle();
confirmation=list.querySelector('.forget-preview');
assert.deepEqual(previewLines(confirmation),[
  'このメモを忘れます。',
  'これを保存した会話の、エージェント自身の会話ファイルには写しが残ります。',
  limits.ja,'この操作は取り消せません。',
]);
const jaCancel=confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_cancel');
assert.equal(text(jaCancel),'やめる');jaCancel.click();
assert.equal(document.activeElement,noteButton);
ctx.ui.setLang('en');

run('setView("sessions")');await run('show()');
list.querySelectorAll('button').find(b=>b.dataset.session===session.key).click();await settle();
const panel=document.getElementById('panel');
const sessionButton=panel.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_session_button');
assert(sessionButton,'the session header has Forget this session');
assert.equal(text(sessionButton),'Forget this session');
sessionButton.click();
assert.deepEqual(requests.at(-1).body,{item:{kind:'session',id:'a1b2c3d4e5f60718'}},'session Forget uses the key, not the displayed session id');
requests.at(-1).resolve(json({...preview,written:3}));await settle();
confirmation=panel.querySelector('.forget-preview');
assert.deepEqual(previewLines(confirmation),[
  'This forgets the session: 7 records, and the 3 memories, 4 cards and 0 summaries made from them.',
  'The 3 memories written with those cards lose their text until they are curated again.',
  "The agent's own transcript files keep their copy: delete it there too if it must go.",
  limits.en,'This cannot be undone.',
]);
confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
start=requests.at(-1);
assert.deepEqual(start.body,{item:{kind:'session',id:session.key},preview_key:'a'.repeat(64),confirmed:true});
const beforeSessionDone=requests.length;
sessions=[];start.resolve(json(started));await settle();
assert.equal(run('ses.open'),null,'forgetting a session goes back to the list');
assert(requests.slice(beforeSessionDone).some(r=>r.url.startsWith('/api/sessions?')),'the sessions list is read again, bypassing the Back cache');
assert(!list.querySelectorAll('button').some(b=>b.dataset.session===session.key),'the server no longer lists the forgotten session');
assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.');

for(const [fresh,paged] of [
  [null,false],
  [{...session,title:'Continue with the parser.'},false],
  [{...session,title:'Continue with the parser.'},true],
]){
  sessions=[session];items=[prompt];run('setView("sessions")');await run('show()');
  list.querySelectorAll('button').find(b=>b.dataset.session===session.key).click();await settle();
  assert.equal(text(document.getElementById('heading')),'Build the parser.');
  panel.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button').click();
  requests.at(-1).resolve(json(preview));await settle();
  panel.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
  start=requests.at(-1);const beforePromptDone=requests.length;
  sessions=paged?[{...session,key:'cccccccccccccccc',started:2}]:fresh?[fresh]:[];
  sessionNext=paged?'older':null;olderSessions=paged?[fresh]:[];
  items=[];start.resolve(json(started));await settle();
  assert(requests.slice(beforePromptDone).some(r=>r.url.startsWith('/api/sessions?')),
    'forgetting a prompt in an open session reads the sessions again');
  if(fresh){
    assert.equal(run('ses.open?.key'),session.key,'a session still returned by the server stays open');
    assert.equal(text(document.getElementById('heading')),'Continue with the parser.','the heading comes from the fresh session entry');
    if(paged)assert(requests.slice(beforePromptDone).some(r=>r.url.includes('before=older')),
      'a session on a later page is found before deciding it is gone');
  }else{
    assert.equal(run('ses.open'),null,'forgetting the last prompt returns to the sessions list');
    assert.equal(text(document.getElementById('heading')),'Sessions');
    assert(!list.querySelectorAll('button').some(b=>b.dataset.session===session.key));
  }
  assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.');
  sessionNext=null;olderSessions=[];
}

ctx.__forgetClaim=claimUid;
const claimPanel=await run('claimPanel(globalThis.__forgetClaim)');panel.replaceChildren(claimPanel);
const claimButton=claimPanel.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
assert(claimButton,'a claim has Forget beside its owner actions');
for(const again of [2,0]){
  claimButton.click();
  assert.deepEqual(requests.at(-1).body,{item:{kind:'claim',id:claimUid}},'a claim uses its full uid');
  requests.at(-1).resolve(json({...preview,again}));await settle();
  confirmation=claimPanel.querySelector('.forget-preview');
  assert.deepEqual(previewLines(confirmation),[
    'This forgets the memory.',
    ...(again?['It replaced 2 older memories; they become current again unless another memory replaces them.']:[]),
    'The records it was made from stay, and search still finds them: forget the prompt or the conversation when the text itself must go.',
    limits.en,'This cannot be undone.',
  ]);
  confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_cancel').click();
}
assert.equal(document.activeElement,claimButton);
run('setView("timeline")');items=[prompt,note];await run('show()');
const forgetButtons=list.querySelectorAll('button').filter(b=>b.dataset.i18n==='forget_button');
button=forgetButtons[0];
button.click();
let beforeOther=requests.length;
forgetButtons[1].click();button.click();
assert.equal(requests.length,beforeOther,'a preview in flight blocks other Forget requests');
requests.at(-1).resolve(json(preview));await settle();
forgetButtons[1].click();
assert.equal(list.querySelector('.forget-preview'),null,'opening another preview closes the previous one');
assert(!button.disabled,'the previous opener is usable again');
assert.equal(requests.at(-1).body.item.kind,'note');
requests.at(-1).resolve(json(preview));await settle();
assert.equal(list.querySelectorAll('.forget-preview').length,1,'only one confirmation is open');
confirmation=list.querySelector('.forget-preview');
confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
start=requests.at(-1);beforeOther=requests.length;
button.click();
assert.equal(requests.length,beforeOther,'a start in flight blocks another item');
for(const cb of confirmation.listeners.keydown||[])cb({key:'Escape',preventDefault(){throw new Error('busy Escape must not close');}});
assert.equal(list.querySelector('.forget-preview'),confirmation,'a start cannot be dismissed by Escape');
start.reject(new Error('connection lost'));await settle();
assert.equal(document.getElementById('status').textContent,
  'The connection ended before the result was confirmed. The operation may already be recorded. Check the stored state before repeating it.');
assert.equal(requests.length,beforeOther,'a lost result is not retried');
assert.equal(list.querySelector('.forget-preview'),null);
button.click();const failedPreview=requests.at(-1);
forgetButtons[1].focus();
failedPreview.reject(new Error('network failure'));await settle();
assert.equal(document.getElementById('status').textContent,'The viewer could not be reached. Please try again.');
assert(!button.disabled,'a rejected preview enables Forget again');
assert.equal(document.activeElement,button,'a rejected preview restores focus to Forget');
assert.equal(requests.at(-1),failedPreview,'a rejected preview is not retried');
assert.equal(list.querySelector('.forget-preview'),null,'a rejected preview opens no confirmation');
const narrative=list.querySelector('.feed-narrative');
list.querySelectorAll('button').find(b=>b.dataset.i18n==='narrative').click();
assert(!narrative.hidden,'an unrelated note is open before the refusals');
for(const [code,status,message] of [
  ['forget_item',400,'This cannot be forgotten from here.'],
  ['forget_preview_failed',422,'What it would forget could not be read.'],
  ['forget_too_large',422,'This takes too many records to forget from here: forget its whole conversation instead.'],
  ['forget_other_device',422,'This prompt was recorded by another device: forget its whole conversation instead.'],
]){
  button.click();const count=requests.length;
  requests.at(-1).resolve(json({code,field:'item'},status));await settle();
  assert.equal(document.getElementById('status').textContent,message);
  assert.equal(requests.length,count,'a preview refusal sends nothing more');
  assert.equal(list.querySelector('.forget-preview'),null);
  assert(list.contains(button)&&!button.disabled,'a preview refusal leaves the item usable');
  assert(list.contains(narrative)&&!narrative.hidden,'a preview refusal leaves the unrelated open note alone');
}
for(const [code,status,message] of [
  ['forget_item',422,'This cannot be forgotten from here.'],
  ['forget_confirmation',422,'It was not forgotten.'],
  ['forget_key',422,'It was not forgotten.'],
  ['forget_failed',500,'It was not forgotten.'],
  ['forget_too_large',422,'This takes too many records to forget from here: forget its whole conversation instead.'],
  ['forget_other_device',422,'This prompt was recorded by another device: forget its whole conversation instead.'],
]){
  button.click();requests.at(-1).resolve(json(preview));await settle();
  confirmation=list.querySelector('.forget-preview');
  confirmation.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
  const count=requests.length;
  requests.at(-1).resolve(json({code,field:'item'},status));await settle();
  assert.equal(document.getElementById('status').textContent,message);
  assert.equal(requests.length,count,'a start refusal does not reload or retry');
  assert.equal(list.querySelector('.forget-preview'),null);
  assert(list.contains(button)&&!button.disabled,'a start refusal leaves the item usable');
  assert(list.contains(narrative)&&!narrative.hidden,'a start refusal leaves the unrelated open note alone');
}
for(const answer of [json(preview),json({code:'forget_too_large'},422)]){
  button.click();const late=requests.at(-1);
  await run('show()');
  run('setStatus("Current page status")');
  late.resolve(answer);await settle();
  assert.equal(list.querySelector('.forget-preview'),null,'a preview for a replaced view is discarded');
  assert.equal(document.getElementById('status').textContent,'Current page status','a late preview refusal leaves the current status unchanged');
  button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
}
for(const [stage,outcome] of [
  ['preview','success'],['preview','refusal'],['preview','network'],['start','refusal'],['start','network'],
]){
  button.click();
  if(stage==='start'){
    requests.at(-1).resolve(json(preview));await settle();
    list.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
  }
  const pending=requests.at(-1);
  assert.equal(pending.url,`/api/forget/${stage}`);
  document.getElementById('q').value='';
  run('setView("records")');const loading=run('show()');
  const recordsRead=requests.at(-1);
  assert(recordsRead.url.startsWith('/api/timeline?'));
  assert(button.isConnected,'the old control remains attached while the next page loads');
  run('setStatus("Current page status")');document.getElementById('status').focus();
  if(outcome==='network')pending.reject(new Error('late failure'));
  else pending.resolve(outcome==='success'?json(preview):json({code:'forget_too_large'},422));
  await settle();
  assert.equal(document.getElementById('status').textContent,'Current page status',`${stage}/${outcome}: a late answer leaves the new status alone`);
  assert.equal(document.activeElement,document.getElementById('status'),`${stage}/${outcome}: a late answer does not steal focus`);
  assert.equal(list.querySelector('.forget-preview'),null,'a late answer leaves no confirmation in the old page');
  recordsRead.resolve(json({items:[],next:null}));await loading;
  run('setView("timeline")');await run('show()');
  button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
}
button=list.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_button');
button.click();requests.at(-1).resolve(json(preview));await settle();
list.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
start=requests.at(-1);
await run('show()');
const beforeLateStart=requests.length;
start.resolve(json(started));await settle();
assert(requests.slice(beforeLateStart).some(r=>r.url.startsWith('/api/feed?')),
  'a successful start after Refresh still reads the current feed again');
assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.');
const newer={...session,key:'bbbbbbbbbbbbbbbb',title:'A newer session',started:2};
sessions=[newer];sessionNext='older';olderSessions=[session];
run('setView("sessions")');await run('show()');
panel.querySelectorAll('button').find(b=>b.dataset.i18n==='more').click();await settle();
list.querySelectorAll('button').find(b=>b.dataset.session===session.key).click();await settle();
panel.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_session_button').click();
requests.at(-1).resolve(json(preview));await settle();
panel.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
start=requests.at(-1);
panel.querySelectorAll('button').find(b=>b.className.includes('session-back')).click();await settle();
list.querySelectorAll('button').find(b=>b.dataset.session===newer.key).click();await settle();
olderSessions=[];start.resolve(json(started));await settle();
assert.equal(run('ses.open.key'),newer.key,'a late success keeps a different session open');
const beforeBack=requests.length;
panel.querySelectorAll('button').find(b=>b.className.includes('session-back')).click();await settle();
assert(requests.slice(beforeBack).some(r=>r.url.startsWith('/api/sessions?')),'Back rereads after a background forget');
assert(!list.querySelectorAll('button').some(b=>b.dataset.session===session.key),
  'a forgotten session on an older page does not survive in the Back cache');
sessionNext=null;
sessions=[session];run('setView("sessions")');await run('show()');
list.querySelectorAll('button').find(b=>b.dataset.session===session.key).click();await settle();
panel.querySelectorAll('button').find(b=>b.dataset.i18n==='forget_session_button').click();
requests.at(-1).resolve(json(preview));await settle();
panel.querySelector('.forget-preview').querySelectorAll('button').find(b=>b.dataset.i18n==='forget_confirm').click();
start=requests.at(-1);
ctx.ui.setView('settings');
const draft={unsaved:'keep this draft'};ctx.ui.setForm(draft);
const fields=new Node();fields.className='settings';
const input=new Node('input');input.value='not saved';fields.append(input);panel.replaceChildren(fields);
const beforeSettings=requests.length;
start.resolve(json(started));await settle();
assert.equal(run('view'),'settings','a late session result does not take the person back from another tab');
assert(requests.slice(beforeSettings).some(r=>r.url.startsWith('/api/repos?')),'a late success uses the current view refresh path');
assert.equal(ctx.ui.getForm(),draft,'the refresh preserves unsaved settings');
assert.equal(input.value,'not saved');
assert(panel.contains(input),'the current settings fields stay on the page');
assert.equal(document.getElementById('status').textContent,'Forgotten. The stores are cleaned in the background.');
ctx.ui.setForm(null);run('setView("timeline")');
assert(requests.filter(r=>r.options.method).every(r=>r.options.method==='POST'),'every write is a POST');
console.log('PASS: Forget: prompt/note/session/claim, preview counts, Cancel/Escape focus, confirmed POST, server reread, stale/refusals, one at a time and JA/EN');
}

// Parity row 3 (docs/card-hide.md): a card or a summary is removed from the page, Undo brings it
// back, a refusal is named, and Settings lists what was removed to bring it back.
{
const run=code=>vm.runInContext(code,ctx);
const settle=async()=>{for(let i=0;i<3;i++)await new Promise(resolve=>setImmediate(resolve));};
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const json=(value,status=200)=>({ok:status===200,status,headers:{get:()=> 'application/json'},json:async()=>structuredClone(value)});
const card={kind:'card',id:'12.0',ts:30,agent:'claude',repo_name:'fern',repo:'example.test/team/fern',type:'feature',
  title:'Azimuth parser',subtitle:'',narrative:'',facts:[],concepts:[],files_read:[],files_modified:[]};
const summary={kind:'summary',id:'S13',ts:40,agent:'claude',repo_name:'fern',repo:'example.test/team/fern',
  fields:{request:'Fix the parser.'}};
const requests=[];
// holdFeed: a feed read waits for the check to answer it; noisy: a reread writes the status line,
// as a view's own reread may.
let items=[summary,card],hidden={items:[]},holdFeed=false,noisy=false;
ctx.fetch=(url,options={})=>{
  const request={url,options,body:options.body?JSON.parse(options.body):null};requests.push(request);
  if(options.method==='POST'||holdFeed&&url.startsWith('/api/feed?'))
    return new Promise((resolve,reject)=>Object.assign(request,{resolve,reject}));
  if(url.startsWith('/api/feed?')){if(noisy)run("setStatus('Reading the feed.')");return Promise.resolve(json({items,next:null}));}
  if(url.startsWith('/api/repos?'))return Promise.resolve(json({current:'',repos:[]}));
  if(url.startsWith('/api/hidden?'))return Promise.resolve(hidden?json(hidden):json({code:'hide_unavailable'},503));
  if(url.startsWith('/api/settings?'))return Promise.resolve(json(validSettings()));
  if(url.startsWith('/api/privacy?'))return Promise.resolve(json({available:false,repositories:[],rescan:{state:'unavailable'}}));
  if(url.startsWith('/api/setup?')||url.startsWith('/api/embedding?'))return new Promise(()=>{});
  throw new Error(`unexpected read: ${url}`);
};
ctx.ui.setLang('en');run('reposLoaded=true;setView("timeline")');
await run('show()');
const list=document.getElementById('list');
const status=document.getElementById('status');
// As a browser's: setting the text clears what was appended after it.
Object.defineProperty(status,'textContent',{configurable:true,get(){return this.shown??'';},set(value){this.shown=value;this.children=[];}});
const removeOf=id=>list.querySelectorAll('footer').find(f=>text(f).includes(id))
  ?.querySelectorAll('button').find(b=>b.dataset.i18n==='hide_button');
assert(removeOf('12.0')&&removeOf('S13'),'a card and a summary each have Remove from the page');
assert(!list.querySelectorAll('button').some(b=>b.dataset.i18n==='forget_button'),'neither has Forget');
removeOf('12.0').click();
let post=requests.at(-1);
assert.equal(post.url,'/api/hide');
assert.deepEqual(post.body,{item:{kind:'card',id:'12.0'},hidden:true});
items=[summary];noisy=true;
post.resolve(json({state:'applied',hidden:true}));await settle();
noisy=false;
assert(!list.querySelectorAll('footer').some(f=>text(f).includes('12.0')),'the page reads again without the card');
assert.equal(status.textContent,'Removed from the page. Undo it here, or later under Settings, Removed from the page.');
let undo=status.children.filter(b=>b?.dataset?.i18n==='hide_undo').at(-1);
assert(undo,'the result offers Undo');
run("setStatus('')");
assert.equal(status.textContent,'Removed from the page. Undo it here, or later under Settings, Removed from the page.',
  'a redraw keeps the result');
assert(status.children.filter(b=>b?.dataset?.i18n==='hide_undo').at(-1)===undo,'and its Undo');
undo.click();
post=requests.at(-1);
assert.deepEqual(post.body,{item:{kind:'card',id:'12.0'},hidden:false});
items=[summary,card];noisy=true;
post.resolve(json({state:'applied',hidden:false}));await settle();
noisy=false;
assert(removeOf('12.0'),'Undo brings the card back');
assert.equal(status.textContent,'Brought back to the page.','through a reread that writes the status line');
// Undo pressed before the removal's reread answered: the removal's late result does not cover it.
removeOf('12.0').click();
holdFeed=true;items=[summary];
requests.at(-1).resolve(json({state:'applied',hidden:true}));await settle();
const removalRead=requests.at(-1);
assert(removalRead.url.startsWith('/api/feed?'),'the removal reads the feed again');
status.children.find(b=>b?.dataset?.i18n==='hide_undo').click();
requests.at(-1).resolve(json({state:'applied',hidden:false}));await settle();
const undoRead=requests.at(-1);
assert(undoRead.url.startsWith('/api/feed?')&&undoRead!==removalRead);
holdFeed=false;items=[summary,card];
undoRead.resolve(json({items,next:null}));await settle();
removalRead.resolve(json({items,next:null}));await settle();
assert.equal(status.textContent,'Brought back to the page.','Undo\'s result stays');
// An Undo refused while the removal's reread waits: the removal's late result does not hide why.
removeOf('12.0').click();
holdFeed=true;items=[summary];
requests.at(-1).resolve(json({state:'applied',hidden:true}));await settle();
const lateRead=requests.at(-1);
status.children.find(b=>b?.dataset?.i18n==='hide_undo').click();
requests.at(-1).resolve(json({code:'hide_unavailable'},503));await settle();
assert.equal(status.textContent,'The memory is busy. Try again in a moment.');
holdFeed=false;
lateRead.resolve(json({items,next:null}));await settle();
assert.equal(status.textContent,'The memory is busy. Try again in a moment.','the refusal stays');
items=[summary,card];await run('show()');
// A refusal is named; a summary is removed by its own kind; Japanese.
ctx.ui.setLang('ja');await run('show()');
removeOf('S13').click();
post=requests.at(-1);
assert.deepEqual(post.body,{item:{kind:'summary',id:'S13'},hidden:true});
post.resolve(json({code:'hide_not_found',field:'item'},404));await settle();
assert.equal(status.textContent,'すでにありません。ページを読み込み直してください。');
removeOf('S13').click();
requests.at(-1).resolve(json({state:'pending',hidden:true,code:'hide_pending'}));await settle();
assert.equal(status.textContent,'受け付けました。記憶の更新が追いつくと、ページに反映されます。');
// Settings lists what was removed, and brings it back.
ctx.ui.setLang('en');
hidden={items:[{kind:'summary',id:'S13',title:'Fix the parser.',repo_name:'fern',ts:40}]};
ctx.ui.setView('settings');
const render=await ctx.ui.showSettings();render();
assert.deepEqual(ctx.ui.getForm().hidden,hidden,'opening Settings reads what was removed');
ctx.ui.setForm(null);run('setView("timeline")');
const f={hidden:{items:[{kind:'card',id:'12.0',title:'Azimuth parser',repo_name:'fern',ts:30}]}};
const section=run('hiddenSection')(f);
assert.equal(text(section.querySelector('h3')),'Removed from the page');
assert(text(section).includes('Azimuth parser')&&text(section).includes('card')&&text(section).includes('fern'));
assert.equal(run('hiddenSection')({hidden:{items:[]}}).querySelectorAll('p').at(-1).dataset.i18n,'hidden_none');
assert.equal(run('hiddenSection')({hidden:null}).querySelectorAll('p').at(-1).dataset.i18n,'hidden_unavailable');
ctx.ui.setView('settings');ctx.ui.setForm(f);
// The whole page is drawn by other checks: here, only that the list is drawn again.
const drawSettings=run('drawSettings');let drawn=0;ctx.drawSettings=()=>{drawn++;};
const restore=section.querySelectorAll('button').find(b=>b.dataset.i18n==='hidden_restore');
hidden={items:[]};
restore.click();
post=requests.at(-1);
assert.deepEqual(post.body,{item:{kind:'card',id:'12.0'},hidden:false});
post.resolve(json({state:'applied',hidden:false}));await settle();
assert(requests.some(r=>r.url.startsWith('/api/hidden?')),'the list is read again');
assert.deepEqual(f.hidden,{items:[]});
assert.equal(drawn,1,'Settings is drawn again with the list read');
assert.equal(status.textContent,'Brought back to the page.');
ctx.drawSettings=drawSettings;ctx.ui.setForm(null);run('setView("timeline")');
delete status.textContent;status.children=[];
// #661: past the first page, Show more reads the next one, after the last of each kind shown, and
// adds it below; the end of the list has none, and the simple page has it too.
{
  const next='{"cards":[30,"d",12,0],"summaries":null}';
  const g={hidden:{items:[{kind:'card',id:'12.0',title:'Azimuth parser',repo_name:'fern',ts:30}],next}};
  ctx.ui.setView('settings');ctx.ui.setForm(g);
  let drawnMore=0;ctx.drawSettings=()=>{drawnMore++;};
  const showMore=node=>node.querySelectorAll('button').find(b=>b.dataset.i18n==='more');
  const more=showMore(run('hiddenSection')(g));
  assert(more,'a list with a next page has Show more');
  hidden={items:[{kind:'summary',id:'S9',title:'Older summary',repo_name:'fern',ts:20}],next:null};
  more.click();await settle();
  const asked=requests.filter(r=>r.url.startsWith('/api/hidden?')).at(-1);
  assert.equal(new URLSearchParams(asked.url.split('?')[1]).get('before'),next,'it reads after the last shown');
  assert.deepEqual(Array.from(g.hidden.items,i=>i.id),['12.0','S9'],'the next page is added below');
  assert.equal(g.hidden.next,null);
  assert.equal(drawnMore,1,'Settings is drawn again with both pages');
  assert(!showMore(run('hiddenSection')(g)),'no Show more at the end');
  const listed=run('hiddenSection')({hidden:{items:g.hidden.items,next}});
  assert(showMore(listed)&&text(showMore(listed))==='More','Show more is worded');
  ctx.ui.setLang('ja');
  assert.equal(text(showMore(run('hiddenSection')({hidden:{items:g.hidden.items,next}}))),'もっと見る');
  ctx.ui.setLang('en');
  // A page that cannot be read leaves the list as it was and says so.
  const h={hidden:{items:g.hidden.items,next}};ctx.ui.setForm(h);
  hidden=null;
  const again=showMore(run('hiddenSection')(h));again.click();await settle();
  assert.deepEqual(Array.from(h.hidden.items,i=>i.id),['12.0','S9'],'what was listed stays');
  assert.equal(again.disabled,false,'and it can be asked again');
  assert.equal(status.textContent,'What was removed could not be read.');
  // A Bring back while a page is read reads the list from its start: the page read for the list
  // before is dropped.
  {
    const k={hidden:{items:[g.hidden.items[0]],next}};ctx.ui.setForm(k);
    const prior=ctx.fetch;let release;
    ctx.fetch=(url,options)=>url.startsWith('/api/hidden?before=')
      ?new Promise(resolve=>{release=()=>resolve(json({items:[{kind:'card',id:'old',title:'Old',repo_name:'fern',ts:1}],next:null}));})
      :prior(url,options);
    showMore(run('hiddenSection')(k)).click();
    k.hidden={items:[],next:null};
    release();await settle();
    assert.deepEqual(Array.from(k.hidden.items,i=>i.id),[],'the page read for the list before is dropped');
    ctx.fetch=prior;
  }
  // #679: a Bring back from a later page leaves the first page and its next position as they were;
  // the page read for the list before is dropped all the same.
  {
    const k={hidden:{items:[g.hidden.items[0]],next}};ctx.ui.setForm(k);
    const prior=ctx.fetch;let release;
    ctx.fetch=(url,options)=>url.startsWith('/api/hidden?before=')
      ?new Promise(resolve=>{release=()=>resolve(json({items:[{kind:'card',id:'back',title:'Back',repo_name:'fern',ts:1}],next:null}));})
      :prior(url,options);
    showMore(run('hiddenSection')(k)).click();
    k.hidden={items:[g.hidden.items[0]],next};
    release();await settle();
    assert.deepEqual(Array.from(k.hidden.items,i=>i.id),['12.0'],'a page read before a reread with the same next is dropped');
    ctx.fetch=prior;
  }
  hidden={items:[]};
  ctx.drawSettings=drawSettings;ctx.ui.setForm(null);run('setView("timeline")');
  delete status.textContent;status.children=[];
}
assert(requests.filter(r=>r.options.method).every(r=>r.options.method==='POST'),'every write is a POST');
console.log('PASS: Remove from the page: card and summary, Undo through redraws, refusals named, JA/EN, Settings list, Show more and Bring back');
}

// Decision 57 slice 5b (docs/my-work-model.md V1): the status line, in English; My Work's, read on
// its own. My Goals' page went with slice 10c-3b.
{
const run=code=>vm.runInContext(code,ctx);
const panel=document.getElementById('panel');
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const day=Date.UTC(2026,9,27,12,0);
run('lang="en"');
const line=status=>text(run(`myWorkStatusLine(${JSON.stringify(status)})`));
assert.equal(line({state:'off'}),'Off: My Work is not written with AI. Open Settings');
assert.equal(line({state:'summaries_off'}),'On, but waiting: summaries are off. Open Settings');
assert(line({state:'waiting',why:'bound',until:day,calls_a_day:100}).startsWith('Waiting: the daily limit of 100 calls is reached. Next try:'),'the daily bound');
assert(line({state:'waiting',why:'budget',until:day}).startsWith('Waiting: an AI provider’s spending or call limit is reached. Next try:'),'a budget');
assert(line({state:'waiting',why:'resting',until:day}).startsWith('Waiting: every AI provider is resting. Next try:'),'every entry resting');
assert.equal(line({state:'waiting',why:'owner',until:day}),'Waiting: no AI provider can take it. Check the AI providers in Settings.');
assert(line({state:'written',at:day}).startsWith('Written'),'when it wrote');
assert.equal(line({state:'none'}),'Nothing written yet.');
assert.equal(run('myWorkStatusLine(null)'),null,'no line when the status did not read');
// My Work's status line comes by a read of its own, after the requests'.
ctx.fetch=url=>Promise.resolve(response(String(url).includes('my-work/status')?{state:'none'}:{requests:[],unlinked:0}));
run('ui.setView("my_work")');
(await run('showMyWork()'))();
await new Promise(resolve=>setTimeout(resolve,0));
assert(text(panel).includes('Nothing written yet.'),'My Work shows its status line once it reads');
// #722: of two reads of My Work's status line in flight, the later one's answer stays.
const pending=[];
ctx.fetch=()=>new Promise(resolve=>pending.push(resolve));
run('ui.setView("my_work");mw.status=null;myWorkLineSlot()');
const older=run('myWorkLineRead()');const newer=run('myWorkLineRead()');
pending[1](response({state:'written',at:day}));await newer;
pending[0](response({state:'none'}));await older;
assert(text(run('mw.line')).startsWith('Written'),'an older read landing later is dropped');
console.log('PASS: The status line: each state, My Work\'s read on its own, the latest line');
}

// Decision 57 slice 6b (docs/my-work-model.md V2): the status words an area's header shows, by
// claude-mem's statusOf over My Work's states, and asked again by full dates in UTC; slice 7a's
// build story.
{
const run=code=>vm.runInContext(code,ctx);
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const day=Date.UTC(2026,9,20,12,0);
const req=(id,status,progress=null)=>({id,title:id,status,progress,updated:day,checked:null,work:[],sessions:[]});
run('lang="en"');
const status=(rows,last=day)=>{ctx.__g={last};ctx.__rows=rows;return run('myGoalsStatus(globalThis.__g,globalThis.__rows)');};
assert.equal(status([]),'nothing');
assert.equal(status([req('a','needs'),req('b','working')]),'your_move');
assert.equal(status([req('a','ready'),req('b','shipped')]),'moving');
assert.equal(status([req('a','checked')]),'your_move','a checked request is the owner\'s move, as claude-mem');
assert.equal(status([req('a','quiet')]),'stalled');
const shipped=done=>req('a','shipped',{percent:100,reviewed:null,completed:done});
assert.equal(status([shipped('2026-10-25')]),'shipped');
assert.equal(status([shipped('2026-10-19')]),'asked_again');
// asked_again_after_ship_compares_full_dates (V2): asked in a later year, on an earlier month and
// day than the ship's, is asked again; claude-mem's month and day alone would say shipped.
assert.equal(status([shipped('2025-10-27')]),'asked_again');
// #731: asked again compares UTC days, as the ship day is recorded: in Tokyo an ask at 22:00 UTC
// is the next local day, and a ship an hour later is not before it.
const tz=process.env.TZ;process.env.TZ='Asia/Tokyo';
assert.equal(status([shipped('2026-10-20')],Date.UTC(2026,9,20,22,0)),'shipped','one time zone');
if(tz===undefined)delete process.env.TZ;else process.env.TZ=tz;
// Slice 7a (P6, V4): the build story at My Work's top: nothing before one is written, then the
// recap and the chapters behind one disclosure, the newest week first.
run('mw.story=null');
assert.equal(run('myWorkBuildStory()'),null,'no story yet');
ctx.__story={recap:'It ships weekly.',chapters:[{week:'2026-10-19',text:'The parser shipped.\n\nIts tests came next.'},{week:'2026-10-12',text:'The parser began.'}]};
run('mw.story=globalThis.__story');
const story=text(run('myWorkBuildStory()'));
assert(story.startsWith('Build story It ships weekly. Week by week · 2'),story);
assert(story.includes('Oct 19 to Oct 25 The parser shipped. Its tests came next. Oct 12 to Oct 18 The parser began.'),story);
run('lang="ja"');
assert(text(run('myWorkBuildStory()')).includes('作業の経緯')&&text(run('myWorkBuildStory()')).includes('週ごと · 2'),'in Japanese');
// #733: the keyboard on its disclosure is reading, so a changed read waits behind "Show updates".
const section=run('myWorkBuildStory()');
section.querySelectorAll('summary')[0].focus();
run('mw.request=null');
assert.equal(run('myWorkReading()'),true,'reading the build story');
run('lang="en";mw.story=null');
console.log('PASS: Status words by My Work\'s states, asked again by full dates in UTC, the build story');
}

// Decision 57 slice 8a (docs/my-work-model.md V3, R3, R4): My Rules: the suggestions waiting with
// approve, edit then approve and reject; your rules with on/off, edit and undo of an approval; the
// rejected ones; add rule; Undo after an approve or a reject; refusals named; JA/EN.
{
const run=code=>vm.runInContext(code,ctx);
const panel=document.getElementById('panel');
const text=node=>(typeof node==='string'?node:(node?.children||[]).map(text).join(' ')).replace(/\s+/g,' ');
const json=(value,status=200)=>({ok:status>=200&&status<300,status,headers:{get:()=>'application/json'},json:async()=>value});
const settle=async()=>{for(let i=0;i<6;i++)await new Promise(resolve=>setImmediate(resolve));};
const day=Date.UTC(2026,9,27,12,0);
for(const [key,[en,ja]] of Object.entries(run('TEXT')).filter(([key])=>key.startsWith('my_rules')||key.startsWith('rule_'))){
  assert(en&&ja&&en!==ja,`${key} has its English and Japanese`);
}
const answer={waiting:[{id:'s1',statement:'Write the tests first.',seen:3,strength:4.5,quotes:[{text:'tests first please',at:day}]},
  {id:'s4',statement:'Keep logs in UTC.',seen:1,strength:0.5,quotes:[]}],
  rejected:[{id:'s2',statement:'Indent with tabs.',seen:1,strength:1,quotes:[]}],
  yours:[{scope:'global',rules:[{uid:'u'.repeat(64),text:'Ship small PRs.',on:true,since:day,suggestion:'s3',seen:2,quotes:[{text:'small PRs',at:day}]},
    {uid:'v'.repeat(64),text:'Answer in Japanese.',on:false,since:day-1,suggestion:null,seen:null,quotes:[]}]}],status:{state:'written',at:day}};
const draw=a=>{ctx.__rules=a;run('mr.answer=globalThis.__rules;mr.editing=null;mr.adding=null;mr.last=null;myRulesDraw()');return text(panel);};
const button=focus=>panel.querySelectorAll('button').find(b=>b.dataset.focus===focus);
const type=(node,value)=>{node.value=value;for(const cb of node.listeners.input||[])cb({target:node});};
const posted=[];let answers=[];
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){posted.push({url,body:JSON.parse(options.body)});return Promise.resolve(answers.shift());}
  return new Promise(()=>{});
};
run('globalThis.__show=show;show=async()=>{};lang="en";ui.setView("my_rules")');
let shown=draw(answer);
assert(shown.includes('Needs your decision')&&shown.includes('Suggested · learned from your requests, not yours until you approve'),'the suggestions');
assert(shown.includes('Seen in 3 requests · Strong')&&shown.includes('Seen in 1 request · Light'),'how often seen, and how strong');
assert(shown.includes('Why · 1 quote')&&shown.includes('tests first please'),'its quotes behind a disclosure');
assert(shown.indexOf('Write the tests first.')<shown.indexOf('Keep logs in UTC.'),'in the answer\'s order, strongest first');
assert(shown.includes('Your rules')&&shown.includes('Learned, approved by you · Seen in 2 requests'),'an approved rule');
assert(shown.includes('Set by you · Off')&&shown.includes('You wrote this rule, so there are no quotes behind it.'),'one set by the owner, off');
assert(shown.includes('Rejected')&&shown.includes('Indent with tabs.'),'the rejected ones');
assert(shown.includes('reach Claude Code, Codex and Grok at the start of every session'),'where rules go (R4)');
assert(shown.includes('Written'),'the rules job\'s status line');
// Approve, then Undo it from the line it leaves.
answers=[json({state:'applied',uid:'w'.repeat(64)})];
button('approve:s1').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/my-rules',body:{suggestion:'s1',action:'approve',scope:'global'}});
assert(text(panel).includes('Approved. It is now one of your rules.'),'what it did');
assert.equal(run('$("status").textContent'),'Approved. It is now one of your rules.','the message stays through the redraw');
answers=[json({state:'applied'})];
button('last:undo').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/my-rules',body:{suggestion:'s1',action:'undo'}});
assert(!button('last:undo'),'undone');
// Edit, then approve: the edited words are posted.
draw(answer);
button('edit:s1').click();
const draft=panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='draft:s1');
assert.equal(draft.value,'Write the tests first.','the draft starts from the suggestion');
type(draft,'Write the tests before the code.');
answers=[json({state:'applied',uid:'w'.repeat(64)})];
button('save:s1').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s1',action:'approve',scope:'global',statement:'Write the tests before the code.'});
// Reject, with Undo too.
draw(answer);
answers=[json({state:'applied'})];
button('reject:s4').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s4',action:'reject'});
assert(text(panel).includes('Rejected. It stays rejected, even when it comes up again.'));
// Your rules: off and on, edit, undo of an approval; a rejected one undone.
draw(answer);
answers=[json({state:'applied',uid:'u'.repeat(64)})];
button('toggle:'+'u'.repeat(64)).click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/claims/mute',body:{uid:'u'.repeat(64),muted:true}});
assert.equal(button('toggle:'+'v'.repeat(64)).textContent??text(button('toggle:'+'v'.repeat(64))),'Turn rule on');
answers=[json({state:'applied',uid:'v'.repeat(64)})];
button('toggle:'+'v'.repeat(64)).click();await settle();
assert.deepEqual(posted.at(-1).body,{uid:'v'.repeat(64),muted:false});
button('edit:'+'u'.repeat(64)).click();
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='draft:'+'u'.repeat(64)),'Ship small pull requests.');
answers=[json({state:'applied',uid:'u'.repeat(64)})];
button('save:'+'u'.repeat(64)).click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/claims/correct',body:{uid:'u'.repeat(64),body:'Ship small pull requests.'}});
answers=[json({state:'applied'})];
button('undo:s3').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s3',action:'undo'});
answers=[json({state:'applied'})];
button('undo:s2').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s2',action:'undo'});
// Add rule: a preference of every repository.
button('add').click();
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Never use /tmp.');
answers=[json({state:'applied',uid:'x'.repeat(64)})];
button('add:save').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/preferences',body:{text:'Never use /tmp.',apply_to_all_repos:true}});
assert(!panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'the form closes once added');
// Slice 8b: rules by scope, every repository's first; a rule of one repository, approved or added
// with "Applies"; the approved ones whose rule is gone, with Undo; a repository refused.
const repo='github.com/o/r';
const scoped={...answer,repos:[{repo,name:'r'}],removed:[{id:'s5',statement:'Squash merges.',seen:1,strength:1,quotes:[]}],
  yours:[...answer.yours,{scope:'repo',repo,name:'r',rules:[{uid:'y'.repeat(64),text:'Run cargo fmt.',on:true,since:day,suggestion:null,seen:null,quotes:[]}]}]};
shown=draw(scoped);
assert(shown.indexOf('Every repository')<shown.indexOf('Ship small PRs.')&&shown.indexOf('Ship small PRs.')<shown.indexOf('In r')&&shown.indexOf('In r')<shown.indexOf('Run cargo fmt.'),'each scope under its heading, every repository first');
assert(shown.includes('Rule removed')&&shown.includes('Squash merges.'),'an approved one whose rule is gone');
answers=[json({state:'applied'})];
button('undo:s5').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/my-rules',body:{suggestion:'s5',action:'undo'}});
const applies=focus=>panel.querySelectorAll('select').find(n=>n.dataset.focus===focus);
const choose=(node,value)=>{node.value=value;for(const cb of node.listeners.change||[])cb({target:node});};
draw(scoped);
button('edit:s1').click();
assert.equal(applies('applies:s1').value,'','every repository unless chosen');
assert.equal(text(applies('applies:s1')),'Every repository In r','the repositories offered');
choose(applies('applies:s1'),repo);
answers=[json({state:'applied',uid:'w'.repeat(64)})];
button('save:s1').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s1',action:'approve',statement:'Write the tests first.',scope:'repo',repo});
draw(scoped);
button('add').click();
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Run clippy.');
choose(applies('add:applies'),repo);
answers=[json({state:'applied',uid:'z'.repeat(64)})];
button('add:save').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/preferences',body:{text:'Run clippy.',apply_to_all_repos:false,repo}});
draw(scoped);
button('add').click();
assert.equal(applies('add:applies').value,'','a new form applies everywhere again');
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Run clippy.');
choose(applies('add:applies'),repo);
answers=[json({code:'preference_repo_excluded',field:'repo'},422)];
button('add:save').click();await settle();
assert.equal(run('$("status").textContent'),'That repository is not recorded, so no rule is kept for it.');
// #743: each form keeps its own "Applies", and a chosen repository stays when a reread leaves it out.
draw(scoped);
button('edit:s1').click();
choose(applies('applies:s1'),repo);
button('add').click();
assert.equal(applies('add:applies').value,'','the add form starts at every repository');
assert.equal(applies('applies:s1').value,repo,'the edit form keeps its own');
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Run clippy.');
answers=[json({state:'applied',uid:'z'.repeat(64)})];
button('add:save').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/preferences',body:{text:'Run clippy.',apply_to_all_repos:true}});
answers=[json({state:'applied',uid:'w'.repeat(64)})];
button('save:s1').click();await settle();
assert.deepEqual(posted.at(-1).body,{suggestion:'s1',action:'approve',statement:'Write the tests first.',scope:'repo',repo});
draw(scoped);
button('add').click();
choose(applies('add:applies'),repo);
ctx.__rules={...scoped,repos:[]};run('mr.answer=globalThis.__rules;myRulesDraw()');
assert.equal(applies('add:applies').value,repo,'a repository left out of the list stays chosen');
assert(text(applies('add:applies')).includes('In the repository you chose'),'and offered');
assert(!text(applies('add:applies')).includes(repo),'by no text of its name (#745)');
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Run clippy.');
answers=[json({state:'applied',uid:'z'.repeat(64)})];
button('add:save').click();await settle();
assert.deepEqual(posted.at(-1).body,{text:'Run clippy.',apply_to_all_repos:false,repo});
// A refusal is named, and nothing is left as done.
draw(answer);
answers=[json({code:'rule_decided',field:'suggestion'},409)];
button('approve:s1').click();await settle();
assert.equal(run('$("status").textContent'),'That suggestion is already decided. The page reads again.');
assert(!button('last:undo'),'no Undo for a refused approve');
// A rule saved whose decision was not: said, so it is not approved twice.
draw(answer);
answers=[json({state:'pending',uid:'w'.repeat(64),code:'rule_partly_recorded'})];
button('approve:s1').click();await settle();
assert(run('$("status").textContent').startsWith('The rule is saved, but the suggestion still shows as waiting.'));
// Opened quotes stay open through the redraw an action brings.
draw(answer);
const why=panel.querySelectorAll('details').find(d=>d.children[0]?.dataset?.focus==='why:s1');
why.open=true;for(const cb of why.listeners.toggle||[])cb({target:why});
run('myRulesDraw()');
assert.equal(panel.querySelectorAll('details').find(d=>d.children[0]?.dataset?.focus==='why:s1').open,true,'kept open');
// #739: a write that ends after the owner opened another tab draws nothing over it.
draw(answer);
let release;answers=[new Promise(r=>{release=r;})];
button('reject:s4').click();
run('ui.setView("timeline")');panel.replaceChildren();
let reads=0;run('globalThis.__reads=()=>{};show=async()=>{globalThis.__reads();}');ctx.__reads=()=>{reads++;};
run('$("status").textContent=""');
release(json({state:'applied'}));await settle();
assert(!panel.querySelector('.my-rules'),'the other tab keeps its panel');
assert.equal(reads,0,'the other tab is not read again (#741)');
assert.equal(run('$("status").textContent'),'','nor given this page\'s message');
run('show=async()=>{};ui.setView("my_rules")');
// #739: only what was applied is done: a partial add keeps its form open.
draw(answer);
button('add').click();
type(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'Never use /tmp.');
answers=[json({state:'directive_only',code:'preference_partly_recorded'})];
button('add:save').click();await settle();
assert(panel.querySelectorAll('textarea').find(n=>n.dataset.focus==='add:text'),'the form stays open');
// None yet.
shown=draw({waiting:[],rejected:[],yours:[{scope:'global',rules:[]}],status:{state:'off'}});
assert(shown.includes('Nothing waiting. New suggestions show up here after the next learning run.')&&shown.includes('No rules yet'),'none yet');
assert(shown.includes('Off: My Work is not written with AI.'),'and why');
// Japanese.
run('lang="ja"');
shown=draw(answer);
assert(shown.includes('あなたの判断を待っています')&&shown.includes('依頼 3 件で見ました · 強い')&&shown.includes('あなたの規則'),'JA');
shown=draw(scoped);
assert(shown.includes('すべてのリポジトリ')&&shown.includes('r の中だけ')&&shown.includes('外れた規則'),'JA scopes');
run('lang="en";show=globalThis.__show');
console.log('PASS: My Rules 8a: suggestions with approve, edit then approve and reject; your rules with off/on, edit and undo; rejected undone; add rule; Undo; refusals; JA/EN');
console.log('PASS: My Rules 8b: rules by scope; approve and add with Applies; rules removed undone; a repository refused; JA');
}
// Decision 66: in a browser that kept the token from an earlier visit, the page opens from its
// address alone; an address's token is kept once the viewer takes it, a kept token the viewer
// refuses is forgotten, and a refused address token is never kept.
{
const page=async(hash,kept,status,meanwhile=()=>{})=>{
  const store=new Map(kept?[['oboete-token',kept]]:[]);
  const sent=[];
  const c=vm.createContext({document,location:{hash,port:'17373',replaced:[],replace(){}},URL,URLSearchParams,
    navigator:{language:'en'},window:{addEventListener(){}},setInterval(){},TextEncoder,Uint8Array,console,
    localStorage:{getItem:k=>store.get(k)??null,setItem:(k,v)=>store.set(k,String(v)),removeItem:k=>store.delete(k)},
    fetch:(url,options)=>{
      sent.push(options?.headers?.['X-Oboete-Token']);
      meanwhile(store);
      return Promise.resolve({ok:status===200,status,json:async()=>({current:'',repos:[]})});
    }});
  vm.runInContext(source.replace('await start();',''),c,{filename:sourcePath});
  const outcome=await vm.runInContext("api('repos').then(()=>'ok',e=>e.status)",c);
  return [outcome,sent,store.get('oboete-token')??null];
};
const A='a'.repeat(32),K='b'.repeat(32);
assert.deepEqual(await page('',K,200),['ok',[K],K],'the address alone sends the kept token');
assert.deepEqual(await page('',K,401),[401,[K],null],'a kept token the viewer refuses is forgotten');
assert.deepEqual(await page(`#t=${A}`,K,200),['ok',[A],A],'the address token wins and is kept once taken');
assert.deepEqual(await page(`#t=${A}`,K,401),[401,[A],K],'a refused address token is not kept, and the kept one stays');
assert.deepEqual(await page(`#t=${A}`,null,503),[503,[A],null],'nor one the viewer did not answer for');
assert.deepEqual(await page('',null,401),[401,[''],null],'no token anywhere sends none');
const N='c'.repeat(32);
assert.deepEqual(await page('',K,401,store=>store.set('oboete-token',N)),[401,[K],N],
  'a refused kept token leaves a newer one another tab kept meanwhile');
console.log('PASS: decision 66: the kept token opens the page from its address alone; refused, forgotten; an address token kept once taken');
}
// Decision 67 (row X27): the records waiting to be curated, beside the mark as claude-mem shows
// its queue; the mark turns while they are curated and stands still while curation is held; no
// count while curation is off, none are waiting or the viewer does not answer.
{
const run=code=>vm.runInContext(code,ctx);
const reply=value=>({ok:true,status:200,json:async()=>value});
const queue=document.getElementById('queue'),mark=document.getElementById('brand-mark');
let answer={};
ctx.fetch=url=>Promise.resolve(reply(url.startsWith('/api/version')?{v:'q1',...answer}:{current:'',repos:[]}));
document.visibilityState='visible';
const pollWith=async a=>{answer=a;run('polling=false');await run('poll()');};
await pollWith({waiting:3,held:false});
assert.equal(queue.hidden,false,'records waiting show their count');
assert.equal(queue.textContent,'3');
assert.equal(queue.title,'3 records wait to be curated into memory');
assert.equal(queue.attrs['aria-label'],queue.title,'a screen reader is told the count');
assert.equal(queue.className,'queue');
assert.equal(mark.className,'mark-wrap busy','the mark turns while they are curated');
await pollWith({waiting:4,held:true});
assert.equal(queue.textContent,'4');
assert.equal(queue.className,'queue held','a held curation shows its count apart');
assert.equal(mark.className,'mark-wrap','and the mark stands still');
assert.equal(queue.title,'4 records wait: curation is paused until a provider answers, a limit resets or you act on it in Settings');
assert.equal(queue.attrs['aria-label'],queue.title,'and why it waits');
await pollWith({waiting:0,held:false});
assert(queue.hidden,'none waiting, no count');
assert.equal(mark.className,'mark-wrap');
await pollWith({});
assert(queue.hidden,'curation off, no count');
await pollWith({waiting:5,held:false});
document.documentElement=new Node('html');
run("lang='ja';applyLanguage()");
assert.equal(queue.title,'5 件の記録が要約を待っています','the hint follows the language');
run("lang='en';applyLanguage()");
delete document.documentElement;
ctx.fetch=()=>Promise.reject(new Error('down'));
run('polling=false');
await run('poll()');
assert(queue.hidden,'a viewer that does not answer shows no count');
assert.equal(mark.className,'mark-wrap');
delete document.visibilityState;
console.log('PASS: decision 67: the records waiting to be curated beside the mark, turning, held, off and unreachable');
}
