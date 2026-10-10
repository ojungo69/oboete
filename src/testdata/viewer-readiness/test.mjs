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
  focus(){this.focused=true;}
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
  stopped:null,inject:{session_start:true,session_start_note:true,per_prompt:false,correction:true,session_start_chars:6000,
    per_prompt_chars:1500,correction_chars:800},capture:{store_prompts:false,tool_output:'full'},backup:{dir:null},
  redaction:{extra_rules:[],allowlist:[]},chain:[],providers:[],warnings:[],key_input:null,
  ranges:{session_start_chars:[0,20000],per_prompt_chars:[0,5000],correction_chars:[0,2000],window_tokens:[1000,100000],
    idle_minutes:[1,120],paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535]},
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
  stopped:null,inject:{session_start:true,session_start_note:true,per_prompt:false,correction:true,session_start_chars:6000,
    per_prompt_chars:1500,correction_chars:800},capture:{store_prompts:false,tool_output:'full'},backup:{dir:null},
  redaction:{extra_rules:[],allowlist:[]},chain:[],providers:[],warnings:[],key_input:true,
  ranges:{session_start_chars:[0,20000],per_prompt_chars:[0,5000],correction_chars:[0,2000],window_tokens:[1000,100000],
    idle_minutes:[1,120],paid_usd_per_month:[0,null],daily_budget:[1,1000],timeout_s:[1,600],view_port:[1,65535],
    daily_requests:[1,100000]},
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
