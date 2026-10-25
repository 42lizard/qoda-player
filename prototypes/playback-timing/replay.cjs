// Capture the actual walkthroughs from the HTML's portable model; not production tests.
const fs=require('fs'),vm=require('vm'),path=require('path');
const html=fs.readFileSync(path.join(__dirname,'index.html'),'utf8');
const sandbox={module:{exports:{}}};vm.runInNewContext(html.match(/<script id="model">([\s\S]*?)<\/script>/)[1],sandbox);
const model=sandbox.module.exports;
const output=model.cases.map(c=>{let state=model.initial();return {name:c.name,description:c.description,steps:c.steps.map(step=>{for(const action of step.actions)state=model.reduce(state,action);return {label:step.label,state};})};});
fs.writeFileSync(path.join(__dirname,'evidence/walkthroughs.json'),JSON.stringify(output,null,2)+'\n');
console.log(JSON.stringify(output.map(c=>({case:c.name,current:c.steps.at(-1).state.current,quality:c.steps.at(-1).state.quality,reports:c.steps.at(-1).state.reports.length,history:c.steps.at(-1).state.history}))));
