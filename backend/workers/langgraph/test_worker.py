import importlib.util
import pathlib
import tempfile
import sys
import unittest
from langchain_core.messages import AIMessage, ToolMessage
import json
from langgraph.checkpoint.sqlite import SqliteSaver

spec = importlib.util.spec_from_file_location("maestro_worker", pathlib.Path(__file__).with_name("worker.py"))
worker = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = worker
spec.loader.exec_module(worker)

REPORT = dict(status="completed", summary="Inspected file", evidence=["README.md"], risks=[], verdict=None, reasons=[])

class ScriptedModel:
    replies = []
    def __init__(self, **kw): self.model = kw["model"]
    def bind_tools(self, tools, **kw):
        assert kw["strict"] and not kw["parallel_tool_calls"]
        return self
    def invoke(self, messages): return self.replies.pop(0)


def call(name, args, id):
    return AIMessage(content="", tool_calls=[dict(name=name,args=args,id=id,type="tool_call")], usage_metadata={"input_tokens":10,"output_tokens":5,"total_tokens":15})


def init():
    return dict(role="designer",system="Inspect only",prompt="Read README",models=[dict(model="fixed-model",baseUrl="http://unused",apiKeyEnv="UNUSED")],
        tools=[dict(name="read_file",description="Read a file",parameters={"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":False})],
        maxOutputTokens=1024,maxContextChars=24000,maxTurns=8,providerRetries=0,requestTimeoutMs=1000,loopDetectionWindow=3)

class GraphTests(unittest.TestCase):
    def test_image_pixels_reach_model_without_entering_checkpoint_history(self):
        settings=init(); image_id="a"*64
        settings["prompt"] += " [[image:"+image_id+"]]"
        seen=[]
        class VisionModel(ScriptedModel):
            def invoke(self,messages):
                seen.extend(messages)
                return call("submit_report",REPORT,"vision-done")
        def bridge(op,**kw):
            if op=="context":return {"directives":[]}
            if op=="image":
                self.assertEqual(kw["imageId"],image_id)
                return {"url":"data:image/jpeg;base64,PIXELS"}
            if op=="before_model":self.assertNotIn("PIXELS",kw["context"])
            return {}
        with tempfile.TemporaryDirectory() as d, SqliteSaver.from_conn_string(d+"/state.sqlite") as saver:
            result=worker.build_graph(settings,saver,bridge,VisionModel).invoke({"messages":[],"turns":0},{"configurable":{"thread_id":"vision"}})
        self.assertTrue(any(isinstance(m.content,list) and any(b.get("type")=="image_url" for b in m.content) for m in seen))
        self.assertNotIn("PIXELS",repr(result["messages"]))

    def test_save_version_dispatch_returns_receipt_to_native_tool_history(self):
        settings=init()
        settings["tools"]=[dict(name="save_version",description="Save a named code checkpoint",parameters={"type":"object","properties":{"label":{"type":"string"}},"required":["label"],"additionalProperties":False})]
        ScriptedModel.replies=[call("save_version",{"label":"Before edits"},"backup-call"),call("submit_report",REPORT,"done")]
        calls=[]
        def bridge(op,**kw):
            if op=="context":return {"directives":[]}
            if op=="tool":
                calls.append(kw)
                return {"versionId":"saved-by-rust","label":kw["arguments"]["label"],"scope":"task workspace"}
            return {}
        with tempfile.TemporaryDirectory() as d, SqliteSaver.from_conn_string(d+"/state.sqlite") as saver:
            result=worker.build_graph(settings,saver,bridge,ScriptedModel).invoke({"messages":[],"turns":0},{"configurable":{"thread_id":"backup"}})
        self.assertEqual(calls,[{"callId":"backup-call","name":"save_version","arguments":{"label":"Before edits"}}])
        self.assertTrue(any(isinstance(m,ToolMessage) and 'saved-by-rust' in m.content for m in result['messages']))

    def test_compacted_inspection_keeps_receipts_and_complete_recent_exchange(self):
        messages = []
        for i in range(6):
            path = "required-guide.md" if i == 0 else f"component-{i}.tsx"
            messages.extend([call("read_file", {"path": path}, f"read-{i}"),
                ToolMessage(content=json.dumps({"content": "x" * 12000, "path": path, "truncated": False}), tool_call_id=f"read-{i}")])
        history, ledger = worker.context_history(messages, 8000)
        self.assertIn("required-guide.md", ledger)
        self.assertIn('"executions": 1', ledger)
        self.assertEqual(history[-1].tool_call_id, "read-5")
        self.assertIsInstance(history[-2], AIMessage)
        self.assertEqual(history[-2].tool_calls[0]["id"], "read-5")
        self.assertIn("middle omitted", history[-1].content)
        self.assertEqual(len(json.loads(messages[-1].content)["content"]), 12000)
        self.assertNotIn("middle omitted", messages[-1].content)

    def test_receipts_distinguish_failed_reads_and_show_periodic_repetition(self):
        messages = []
        for i, path in enumerate(["a", "b", "c", "a", "b", "c"]):
            messages.extend([call("read_file", {"path": path}, str(i)),
                ToolMessage(content=json.dumps({"error": "not found"}) if path == "b" else json.dumps({"content": "ok", "path": path}), tool_call_id=str(i))])
        _, ledger = worker.context_history(messages, 8000)
        rows = [json.loads(line) for line in ledger.splitlines()]
        self.assertEqual(len(rows), 3)
        self.assertTrue(all(row["executions"] == 2 for row in rows))
        self.assertIn("not found", rows[1]["lastResult"])

    def test_checkpoint_resumes_pending_tool_without_repeating_model(self):
        calls = []
        fail = True
        def bridge(op, **fields):
            nonlocal fail
            calls.append((op,fields))
            if op == "context": return {"directives":[]}
            if op == "tool":
                if fail: raise RuntimeError("approval required")
                return {"content":"real tool result"}
            return {}
        ScriptedModel.replies = [call("read_file",{"path":"README.md"},"read-1"),call("submit_report",REPORT,"report-1")]
        with tempfile.TemporaryDirectory() as directory:
            path = str(pathlib.Path(directory)/"state.sqlite")
            config = {"configurable":{"thread_id":"same-attempt"}}
            with SqliteSaver.from_conn_string(path) as saver:
                graph=worker.build_graph(init(),saver,bridge,ScriptedModel)
                with self.assertRaises(RuntimeError): graph.invoke({"messages":[],"turns":0},config)
                self.assertIn("tools",graph.get_state(config).next)
            fail=False
            with SqliteSaver.from_conn_string(path) as saver:
                graph=worker.build_graph(init(),saver,bridge,ScriptedModel)
                result=graph.invoke(None,config)
                self.assertEqual(result["final"],REPORT)
            self.assertEqual(sum(op=="before_model" for op,_ in calls),2)
            self.assertEqual([f["callId"] for op,f in calls if op=="tool"],["read-1","read-1"])

    def test_block_clarification_is_bounded_and_real_block_preserved(self):
        report={**REPORT,"status":"blocked","summary":"Missing required input"}
        ScriptedModel.replies=[call("submit_report",report,"a"),call("submit_report",report,"b")]
        def bridge(op,**kw): return {"directives":[]} if op=="context" else {}
        with tempfile.TemporaryDirectory() as directory, SqliteSaver.from_conn_string(directory+"/state.sqlite") as saver:
            graph=worker.build_graph(init(),saver,bridge,ScriptedModel)
            result=graph.invoke({"messages":[],"turns":0},{"configurable":{"thread_id":"blocked"}})
            self.assertEqual(result["final"]["status"],"blocked")
            self.assertTrue(result["clarified"])
            self.assertEqual(result["turns"],2)

    def test_unknown_tool_never_reaches_rust_executor(self):
        ScriptedModel.replies=[call("shell",{"command":"echo unsafe"},"bad"),call("submit_report",REPORT,"done")]
        def bridge(op,**kw):
            self.assertNotEqual(op,"tool")
            return {"directives":[]} if op=="context" else {}
        with tempfile.TemporaryDirectory() as directory, SqliteSaver.from_conn_string(directory+"/state.sqlite") as saver:
            result=worker.build_graph(init(),saver,bridge,ScriptedModel).invoke({"messages":[],"turns":0},{"configurable":{"thread_id":"unknown"}})
            self.assertEqual(result["final"]["status"],"completed")

if __name__ == "__main__": unittest.main()
