// THROWAWAY native integration study: real Rust IPC, fictional data, null audio.
import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as C
import Quickshell
import Quickshell.Io

ShellRoot {
 id: root
 property string instance: ""
 property int revision: -1
 property var state: ({queue:[],playing:false,loaded:false,position:0})
 property var pending: ({})
 property var journal: []
 property string clientNonce: Date.now().toString()+"-"+Math.random().toString(36).slice(2)
 property int seq: 0
 property int latestPage: 0
 property int stalePages: 0
 property int cancelledPages: 0
 property int restarts: 0
 property int connections: 0
 property int uncertain: 0
 property int gaps: 0
 property int pageTotal: 0
 property string query: ""
 property string artworkUrl: ""
 property int artVersion: 0
 property real maxHeartbeatMs: 0
 property real lastHeartbeat: Date.now()
 property real pageMs: 0
 property real lastAckMs: 0
 property bool manualDisconnect: false
 function record(kind,detail) { journal=journal.concat([{kind:kind,detail:detail,at:Date.now()}]).slice(-35); }
 function send(o) {o.id=++seq;socket.write(JSON.stringify(o)+"\n");socket.flush();return o.id;}
 function command(op,dropReply) {
  if(!socket.connected || !instance) return -1;
  let o={op:op,instance:instance,operationId:"qml-"+clientNonce+"-"+instance+"-"+(seq+1),trackId:"fixture:track:42",drop_reply:dropReply===true};
  const id=seq+1;let p=Object.assign({},pending);p[id]={op:op,at:Date.now()};pending=p;
  record("pending",op);return send(o);
 }
 function search(text,delay,offset) {query=text;let id=seq+1; latestPage=id;let p=Object.assign({},pending);p[id]={op:"search",at:Date.now()};pending=p;return send({op:"search",query:text,offset:offset,limit:100,delay_ms:delay});}
 function receive(text) {
  const v=JSON.parse(text);
  if(v.type==="snapshot") {
   if(instance && instance!==v.state.instance) {restarts++;record("new-backend","restored paused");}
   instance=v.state.instance; revision=v.state.revision;state=v.state;record("snapshot",revision);
  } else if(v.type==="update") {
   if(v.state.instance!==instance) {record("wrong-instance",v.state.instance);return;}
   if(v.state.revision<=revision) return;
   if(v.state.revision!==revision+1) {gaps++;record("gap","resynchronize");socket.connected=false;return;}
   revision=v.state.revision;state=v.state;
  } else if(v.type==="page" || v.type==="search-cancelled") {
   if(v.type==="search-cancelled") cancelledPages++;
   if(v.id!==latestPage) {stalePages++;record("discarded-page",v.id);}
   else if(v.type==="page") {rows.clear();rows.append(v.rows);pageTotal=v.total;pageMs=Date.now()-(pending[v.id]?.at ?? Date.now());record("page",rows.count);}
  } else if(v.type==="artwork") {artworkUrl=v.url;artVersion=v.version;record("artwork",v.version);}
  else if(v.type==="ack") {lastAckMs=Date.now()-(pending[v.id]?.at ?? Date.now());record("ack",JSON.stringify(v.result));}
  if(v.id!==undefined) {let p=Object.assign({},pending);delete p[v.id];pending=p;}
 }
 function snapshot() {return JSON.stringify({connected:socket.connected,instance:instance,revision:revision,state:state,pending:Object.keys(pending).length,uncertain:uncertain,connections:connections,restarts:restarts,gaps:gaps,rows:rows.count,firstTrack:rows.count?rows.get(0).trackId:"",total:pageTotal,stalePages:stalePages,cancelledPages:cancelledPages,pageMs:pageMs,lastAckMs:lastAckMs,maxHeartbeatMs:maxHeartbeatMs,artVersion:artVersion,artStatus:art.status,artWidth:art.implicitWidth,artworkUrl:artworkUrl,journal:journal});}
 ListModel {id:rows}
 Socket {
  id:socket
  path:Quickshell.env("QODA_PROTO_RUNTIME")+"/backend.sock"
  connected:true
  parser:SplitParser {onRead:data=>root.receive(data)}
  onConnectedChanged: {
   if(connected) {root.connections++; root.send({op:"hello",protocol:1});}
   else {root.uncertain+=Object.keys(root.pending).filter(k=>root.pending[k].op!=="search").length;root.pending={};root.record("disconnected","no mutation replay");}
  }
 }
 Timer {interval:200;repeat:true;running:!root.manualDisconnect;onTriggered:if(!socket.connected) socket.connected=true;}
 Timer {interval:16;repeat:true;running:true;onTriggered:{let now=Date.now();root.maxHeartbeatMs=Math.max(root.maxHeartbeatMs,now-root.lastHeartbeat);root.lastHeartbeat=now;}}
 IpcHandler {
  target:"integration"
  function snapshot():string {return root.snapshot();}
  function command(op:string):int {return root.command(op);}
  function lostReply():int {return root.command("append",true);}
  function search(query:string,delay:int,offset:int):int {return root.search(query,delay,offset);}
  function artwork(version:int):void {root.send({op:"artwork",version:version});}
  function disconnect(value:bool):void {root.manualDisconnect=value;socket.connected=!value;}
  function visible(value:bool):void {window.visible=value;}
  function resetTiming():void {root.maxHeartbeatMs=0;root.lastHeartbeat=Date.now();}
  function quit():void {Qt.quit();}
 }
 FloatingWindow {
  id:window;title:"Qoda · Rust integration prototype";visible:true
  implicitWidth:1000;implicitHeight:650;color:"#151c19"
  ColumnLayout {
   anchors.fill:parent;anchors.margins:18;spacing:12
   Text {text:"QODA / Rust + QML integration study";color:"#c8e6ad";font.pixelSize:21;Layout.fillWidth:true;wrapMode:Text.Wrap}
   Text {text:"Fictional 100,000-track catalog · null audio · throwaway code";color:"#aab8b0";Layout.fillWidth:true;wrapMode:Text.Wrap}
   Text {text:(socket.connected?"Connected":"Disconnected")+" · revision "+root.revision+" · "+(root.state.playing?"Playing to null":"Paused")+" · "+root.state.queue.length+" queue entries · "+Object.keys(root.pending).length+" pending";color:"white";Layout.fillWidth:true;wrapMode:Text.Wrap}
   Text {visible:root.uncertain>0;text:"An edit reply was lost. Queue refreshed; review it before retrying.";color:"#e9be83";Layout.fillWidth:true;wrapMode:Text.Wrap}
   RowLayout {
    C.Button {text:root.state.playing?"Pause":"Play to null";onClicked:root.command("toggle")}
    C.Button {text:"Add fixture";onClicked:root.command("append")}
    C.Button {text:"Reconnect";onClicked:{socket.connected=false}}
   }
   C.TextField {Layout.fillWidth:true;placeholderText:"Search fictional titles";onTextEdited:root.search(text,0,0)}
   Text {text:rows.count+" visible / "+root.pageTotal+" matches · "+root.pageMs+" ms · "+root.stalePages+" old replies ignored";color:"#aab8b0"}
   ListView {
    Layout.fillWidth:true;Layout.fillHeight:true;model:rows;clip:true
    delegate:Rectangle {
     required property int index;required property string title;required property string artist;required property string trackId
     width:ListView.view.width;height:42;color:index%2?"#1a2520":"#1e2b24"
     Text {anchors.verticalCenter:parent.verticalCenter;anchors.left:parent.left;anchors.leftMargin:10;text:title+" / "+artist;color:"#e5ece7"}
    }
   }
   RowLayout {
    Image {id:art;source:root.artworkUrl;sourceSize:Qt.size(48,48);Layout.preferredWidth:48;Layout.preferredHeight:48}
    C.Button {text:"Artwork A";onClicked:root.send({op:"artwork",version:1})}
    C.Button {text:"Artwork B";onClicked:root.send({op:"artwork",version:2})}
    Text {text:"Cache version "+root.artVersion;color:"#aab8b0"}
   }
  }
 }
}
