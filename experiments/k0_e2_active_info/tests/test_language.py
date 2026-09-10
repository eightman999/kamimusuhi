import unittest
from experiments.k0_e2_active_info.language import parse_response,query

class LanguageBoundary(unittest.TestCase):
    def test_valid_json(self):
        self.assertEqual(parse_response('{"category":"B","confidence":0.9,"evidence_id":"record-1"}')['category'],1)
    def test_reject_no_repair(self):
        for text in ['', 'B', '```json {} ```', '{"category":"A","confidence":7,"evidence_id":"record-1"}', '{"category":"A","confidence":NaN,"evidence_id":"record-1"}', '{"category":"A","confidence":true,"evidence_id":"record-1"}', '{"category":"A","confidence":1,"evidence_id":"invented"}', '{"category":["A","B"],"confidence":1,"evidence_id":"record-1"}']:
            with self.subTest(text=text):self.assertFalse(parse_response(text)['valid'])
    def test_retry_once_then_missing(self):
        calls=[]
        def malformed(body,timeout):calls.append(body);return {'choices':[{'message':{'content':'not json'}}]}
        result=query('unused',0,[],transport=malformed);self.assertEqual(len(calls),2);self.assertFalse(result['valid']);self.assertTrue(result['api_success'])
    def test_semantics_distinct_from_parse(self):
        def wrong(body,timeout):return {'choices':[{'message':{'content':'{"category":"B","confidence":1,"evidence_id":"record-1"}'}}]}
        result=query('unused',0,[],transport=wrong);self.assertTrue(result['parse_success']);self.assertFalse(result['semantic_correct'])
    def test_timeout_unavailable(self):
        def unavailable(body,timeout):raise TimeoutError()
        result=query('unused',0,[],transport=unavailable);self.assertFalse(result['api_success']);self.assertEqual(len(result['attempts']),1)
if __name__=='__main__':unittest.main()
