"""Evaluation-only language backend. Serialization happens after the gate opens."""
import json,time,urllib.request,urllib.error
from ..env.signals import SENSOR_NAMES


class RealJ72LanguageBackend:
    def __init__(self,endpoint,model='j72-30m',timeout=10):
        self.endpoint=endpoint.rstrip('/')+'/v1/chat/completions';self.model=model;self.timeout=timeout

    def invoke(self,observation,state):
        event={'event':dict(zip(SENSOR_NAMES,map(float,observation))),'internal_state':state}
        request={'model':self.model,'messages':[{'role':'user','content':'以下のセンサーイベントに短く応答してください。\n'+json.dumps(event,ensure_ascii=False)}],'max_tokens':32,'temperature':0}
        start=time.monotonic()
        try:
            req=urllib.request.Request(self.endpoint,data=json.dumps(request).encode(),headers={'Content-Type':'application/json'})
            with urllib.request.urlopen(req,timeout=self.timeout) as response:
                data=json.loads(response.read(1048576))
            text=data['choices'][0]['message']['content']
            if not isinstance(text,str):raise ValueError('Invalid response content')
            return {'status':'ok','latency':time.monotonic()-start,'response':text[:240]}
        except (OSError,ValueError,KeyError,IndexError,TypeError) as exc:
            return {'status':'LANGUAGE_BACKEND_UNAVAILABLE','latency':time.monotonic()-start,'error_type':type(exc).__name__}
