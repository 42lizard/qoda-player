//@ pragma ShellId qoda-player-lifecycle-study
//@ pragma AppId qoda-player
import QtQuick
import QtQuick.Window
import QtQuick.Controls as C
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
ShellRoot {
 id: root
 property string release: "@RELEASE@"
 property string form: "hidden"
 property string lastForm: "full"
 property bool compatible: false
 property var state: ({playing:false,queue:[],release:""})
 property int seq: 0
 property var events: []
 onFormChanged:events=events.concat([{event:"form",value:form}]).slice(-30)
 function activation(action) {
  events=events.concat([{event:"activation",value:action}]).slice(-30);
  if(action==="hide") form="hidden";
  else if(action==="toggle") form=form==="hidden"?lastForm:"hidden";
  else form=action==="compact"?"compact":"full";
  if(form!=="hidden") lastForm=form;
  if(form==="full") Qt.callLater(()=>{if(full.contentItem.Window.window)full.contentItem.Window.window.requestActivate();});
 }
 Socket {
  id: socket
  path:Quickshell.env("QODA_PROTO_RUNTIME")+"/backend.sock";connected:true
  onConnectedChanged:if(connected){socket.write('{"op":"hello","protocol":1,"id":0}\n');socket.flush();}
  parser:SplitParser {onRead:data=>{
   let v=JSON.parse(data);
   if(v.state){root.state=v.state;root.compatible=v.state.release===root.release;}
  }}
 }
 Timer {interval:200;repeat:true;running:true;onTriggered:if(!socket.connected)socket.connected=true}
 IpcHandler {
  target:"lifecycle"
  function snapshot():string{return JSON.stringify({connected:socket.connected,compatible:root.compatible,release:root.release,form:root.form,lastForm:root.lastForm,shellId:Quickshell.shellId,appId:Quickshell.appId,events:root.events,state:root.state,pid:Quickshell.processId});}
  function present(action:string):string{if(root.compatible)root.activation(action);return JSON.stringify({requested:action,compatible:root.compatible,form:root.form,events:root.events});}
 }
 FloatingWindow {
  id:full;visible:root.form==="full";title:"Qoda lifecycle study · fictional data";implicitWidth:520;implicitHeight:260;color:"#18261d"
  onClosed:{root.events=root.events.concat([{event:"closed",value:root.form}]).slice(-30);root.form="hidden";}
  ColumnLayout {
   anchors.fill:parent;anchors.margins:20
   C.Label {text:"Qoda lifecycle study · "+root.release;color:"#d0e8bb";font.pixelSize:22;Layout.fillWidth:true;wrapMode:Text.Wrap}
   C.Label {text:(root.compatible?"Compatible":"INCOMPATIBLE")+" · "+(root.state.playing?"Null audio playing":"Paused")+" · "+root.state.queue.length+" fictional entries";color:"white";Layout.fillWidth:true;wrapMode:Text.Wrap}
   C.Label {text:root.state.persistenceError ? "Session not saved: "+root.state.persistenceError : "No provider credentials · no real audio";color:"#e7cd9e";Layout.fillWidth:true;wrapMode:Text.Wrap}
   C.Button {text:"Hide study";onClicked:root.form="hidden"}
  }
 }
 PanelWindow {
  visible:root.form==="compact";implicitWidth:340;implicitHeight:120;color:"#18261d"
  anchors {bottom:true;right:true}
  margins {bottom:18;right:18}
  exclusionMode:ExclusionMode.Ignore
  WlrLayershell.namespace:"qoda-lifecycle-study";WlrLayershell.keyboardFocus:WlrKeyboardFocus.None
  C.Label {anchors.centerIn:parent;text:"Qoda study "+root.release+" · null audio";color:"white"}
 }
}
