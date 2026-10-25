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
  remove(){if(this.parentElement)this.parentElement.children=this.parentElement.children.filter(node=>node!==this);this.parentElement=null;}
  addEventListener(name,cb){(this.listeners[name]??=[]).push(cb);}
  setAttribute(name,value){this.attrs[name]=value;}
  removeAttribute(name){delete this.attrs[name];}
  querySelector(selector){return this.querySelectorAll(selector)[0]||null;}
  querySelectorAll(selector){
    if(selector.includes(','))return selector.split(',').flatMap(part=>this.querySelectorAll(part.trim()));
    const tag=selector.toUpperCase();
    return this.children.filter(node=>node&&typeof node==='object').flatMap(node=>[
      ...(node.tagName===tag||selector.startsWith('.')&&node.className.split(' ').includes(selector.slice(1))?[node]:[]),
      ...node.querySelectorAll(selector)]);
  }
  contains(node){return this===node||this.children.some(child=>child&&typeof child==='object'&&child.contains(node));}
  get childNodes(){return this.children;}
  get options(){return this.children.filter(node=>node?.tagName==='OPTION');}
  click(){for(const cb of this.listeners.click||[])cb({target:this});}
  focus(){this.focused=true;document.activeElement=this;}
  select(){this.selected=true;}
  closest(selector){
    for(let node=this;node;node=node.parentElement){
      if(node.tagName===selector.toUpperCase()||selector.startsWith('.')&&node.className.split(' ').includes(selector.slice(1)))return node;
    }
    return null;
  }
}

const ids=new Map();
const document={activeElement:null,createElement:tag=>new Node(tag),getElementById:id=>{
  if(!ids.has(id))ids.set(id,new Node());return ids.get(id);
}};
const ctx=vm.createContext({document,location:{hash:'#t=0123456789abcdef0123456789abcdef',port:'17373',
  replaced:[],replace(url){this.replaced.push(url);}},URL,URLSearchParams,navigator:{language:'en'},
  localStorage:{getItem(){return null;},setItem(){}},window:{addEventListener(){}},
  setInterval(){},TextEncoder,Uint8Array,console});
const sourcePath=fs.realpathSync(process.argv[2]||new URL('../../../assets/viewer/app.js',import.meta.url));
const source=fs.readFileSync(sourcePath,'utf8');
vm.runInContext(source.replace('await start();',''),ctx,{filename:sourcePath});
vm.runInContext('globalThis.ui={showSettings,drawSettings,setView:v=>{view=v;},setForm:v=>{form=v;},getForm:()=>form,setLang:v=>{lang=v;}};',ctx);
// The view the page opens on, before the checks below change it.
const initialView=vm.runInContext('view',ctx);
// The simple page uses the saved chain, including unavailable and disabled entries.
{
const simple=vm.runInContext('({status:simpleProviderStatus,models:simpleProviderModels,listed:catalogHasProvider,chain:simpleSavedChain,catalog:AI_CATALOG,mode:settingsMode})',ctx);
assert.equal(simple.mode,'simple','Settings starts with the simple view');
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
const remembered=[];
ctx.localStorage.setItem=(key,value)=>remembered.push([key,value]);
const switcher=vm.runInContext('settingsViewSwitch()',ctx);
switcher.querySelectorAll('button')[1].click();
assert.deepEqual(remembered.at(-1),['oboete-settings-view','detailed'],'the view choice uses remember');
switcher.querySelectorAll('button')[0].click();
assert.deepEqual(remembered.at(-1),['oboete-settings-view','simple']);
}
// Existing detailed-page checks continue to exercise the whole original page.
vm.runInContext("settingsMode='detailed';",ctx);
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
    idle_minutes:[1,120],paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535],retention_days:[1,36500]},
  view_runtime:{port:17373,mode},...extra});
vm.runInContext('globalThis.w6={formOf,saveBody,applySavedSettings,viewDraft:()=>viewDraft};',ctx);
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
for(const cb of panel.querySelector('form').listeners.submit||[])cb({preventDefault(){}});
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
    daily_requests:[1,100000],retention_days:[1,36500]},
  view_runtime:{port:17373,mode:'foreground'},embedding:embedding(),...extra});
const posted=[];let answers=[];let settingsGets=0;let settingsAnswer=null;let runStatus=null;
ctx.fetch=(url,options)=>{
  if(options?.method==='POST'){posted.push({url,body:JSON.parse(options.body)});return Promise.resolve(answers.shift());}
  if(url.startsWith('/api/settings?')){settingsGets++;return settingsAnswer??Promise.resolve(json(settings()));}
  if(url.startsWith('/api/embedding?')&&runStatus)return Promise.resolve(json(runStatus));
  return new Promise(()=>{});
};
const settle=()=>new Promise(resolve=>setImmediate(resolve));
const panel=document.getElementById('panel');
const section=()=>panel.querySelector('.embedding-settings');
const select=()=>section().querySelectorAll('select').find(node=>node.dataset.field==='embedding.choice');
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
assert.deepEqual(plain(select().options.map(node=>node.value)),['none','local','workers-ai']);
assert.equal(select().value,'none');
select().value='local';for(const cb of select().listeners.change)cb({target:select()});
assert(text(section()).includes('not been downloaded'),'the local files state is shown');
assert(!button('embedding.start'),'no taking before the review');
const consent={choice:'local',dir:'/home/u/.oboete/models/bge-m3',check:false,
  get:[{host:'huggingface.co',bytes:2283937811,source:'model'},{host:'github.com',bytes:9125960,source:'runtime'}]};
const key='ab'.repeat(32);
answers=[json({preview_key:key,consent})];
button('embedding.preview').click();await settle();
assert.deepEqual(posted.at(-1),{url:'/api/embedding/preview',body:{choice:'local'}});
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
assert(select().disabled,'no other method meanwhile');
const stopped={active:null,held:0,last:{choice:'local',phase:'failed',code:'embedding_no_space',held:0,get:2293063771}};
settingsAnswer=Promise.resolve(json({},503));
finish(json(stopped));
await settle();await settle();await settle();
assert(text(section()).includes('not enough free disk space'),'a stopped run says why');
assert(!select().disabled);
// The settings could not be read after it: the run stays watched, and the next poll reads them.
assert(w4.embeddingDraft().watch,'a failed read after the answer keeps the run watched');
settingsAnswer=null;runStatus=stopped;
const before=settingsGets;
await w4.refreshEmbedding();await settle();
assert.equal(settingsGets,before+1,'the next poll reads the settings again');
assert(!w4.embeddingDraft().watch);
runStatus=null;

// A refusal is worded, and points at the field it needs.
select().value='workers-ai';for(const cb of select().listeners.change)cb({target:select()});
answers=[json({code:'no_account',field:'embedding.choice'},422)];
button('embedding.preview').click();await settle();
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
  vm.runInContext("settingsMode='simple';",ctx);
  const ready={chain:[{name:'groq',key:'ok',on:true,effective_model:'openai/gpt-oss-20b',model_rule:'any'}],
    providers:[{name:'groq',saved:{enabled:true}}],stopped:[]};
  for(const [label,s] of [['ready',settings(ready)],['no search section',settings({embedding:undefined})]]){
    f=w4.formOf(s);ctx.ui.setForm(f);ctx.ui.drawSettings();
    const parts=panel.querySelector('.settings').children;
    assert(parts.length>3&&parts.every(part=>part!==null&&part!==undefined),`simple view, ${label}: no absent part`);
  }
  vm.runInContext("settingsMode='detailed';",ctx);
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
assert(text(panel).includes('意味での検索')&&text(section()).includes('このパソコンで処理'),'Japanese section');
ctx.ui.setLang('en');
console.log('PASS: the embedder is reviewed, agreed and run; its values and token');
}

// Simple controls save only their own change, using the existing whole-settings operation.
{
vm.runInContext("settingsMode='simple';globalThis.simpleUi={formOf,TEXT,embeddingDraft:()=>embeddingDraft,renderEmbedding};",ctx);
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
    session_start_chars:[0,20000],per_prompt_chars:[0,5000],per_prompt_wait_ms:[100,3000],correction_chars:[0,2000],window_tokens:[0,100000],idle_minutes:[0,120],retention_days:[1,36500]},
  chain:[{name:'groq',on:true,key:'missing',effective_model:'openai/gpt-oss-120b',model_rule:'any',model:null,
    daily_budget:null,timeout_s:null,differs:[]}],
  providers:[{name:'groq',selector:{source:'builtin',index:0},saved:{enabled:true,kind:'openai',base_url:'https://api.groq.com/openai/v1',
    model:'openai/gpt-oss-120b',limits:{}},effective:{on:true}}]};
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
f.retention.forever=false;f.retention.edit='90';
ctx.ui.setForm(f);ctx.ui.setView('settings');ctx.ui.setLang('ja');ctx.ui.drawSettings();
assert.equal(panel.querySelectorAll('.simple-block').length,7,'all seven blocks are present');
assert(text(panel).includes('要約に使える AI がまだない'),'an unusable home has a clear first action');
assert(!panel.querySelector('form'),'simple settings have no competing whole-page save');
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
assert(text(panel).includes('保存しました'),'a saved receipt is beside the changed block');
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
for(const language of ['ja','en']){
  ctx.ui.setLang(language);ctx.ui.drawSettings();
  for(const [key,pair] of Object.entries(ctx.simpleUi.TEXT).filter(([key])=>key.startsWith('simple_'))){
    assert.equal(pair.length,2,`${key} is bilingual`);assert(pair.every(value=>typeof value==='string'&&value.length));
  }
  assert.equal(panel.querySelectorAll('.simple-block').length,7);
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
vm.runInContext("settingsMode='detailed';",ctx);
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
set([req('x',{updated:now-3*H}),req('y',{updated:now-H}),req('z',{updated:0,title:'A first'})]);
run('mw.range="all";mw.sort="latest";myWorkDraw()');assert.deepEqual(ids(),['y','x','z'],'latest first, no time last');
run('mw.sort="oldest";myWorkDraw()');assert.deepEqual(ids(),['x','y','z'],'oldest first, no time last');
run('mw.sort="title";myWorkDraw()');assert.deepEqual(ids(),['z','x','y'],'A to Z by title');
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
// Slice 4: a request's work product, read when its page opens, once a version; a link only for
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
back();
card('p1').querySelector('button').click();
assert(asked.length===1&&!box().hidden,'the same version shows at once, not read again');
back();
products=[];
set([req('p2')]);
card('p2').querySelector('button').click();
await new Promise(resolve=>setImmediate(resolve));
assert.equal(asked.length,2,'another request is read');
assert(box().hidden,'nothing read, no section');
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
