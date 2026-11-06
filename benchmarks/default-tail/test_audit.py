import copy,unittest
from audit import compare

def model(root_features):
    return {'packages':[{'id':'root','name':'catdb','version':'0.1','source':None},{'id':'dep','name':'dep','version':'1','source':'registry'}],
      'resolve':{'root':'root','nodes':[{'id':'root','features':root_features},{'id':'dep','features':['default']}]}}

class Tests(unittest.TestCase):
    def test_EQ_root_default_marker_is_the_only_permitted_resolution_difference(self):
        self.assertTrue(compare(model(['inline']),model(['default','inline']))['normalized_features_equal'])
    def test_ER_dependency_default_must_not_be_ignored(self):
        b=model(['default','inline']);b['resolve']['nodes'][1]['features']=[]
        with self.assertRaises(AssertionError):compare(model(['inline']),b)
    def test_ER_additional_engine_feature_is_not_equivalent(self):
        with self.assertRaises(AssertionError):compare(model(['inline']),model(['inline','default','skip-history']))
    def test_ER_version_change_is_not_equivalent(self):
        b=model(['default','inline']);b['packages'][1]['version']='2'
        with self.assertRaises(AssertionError):compare(model(['inline']),b)
if __name__=='__main__':unittest.main()
