import unittest
from experiments.k0_f2_interoception_confirmatory.statistics import exact_sign_test,describe,paired_comparison


class StatisticsTests(unittest.TestCase):
    def test_exact_sign_and_minimum_effect(self):
        self.assertEqual(exact_sign_test([1]*12),2/4096)
        self.assertEqual(exact_sign_test([0]*12),1.)
        self.assertTrue(paired_comparison([.02]*12,[0]*12)['pass'])
        self.assertTrue(paired_comparison([.01]*12,[0]*12)['pass'])
        self.assertFalse(paired_comparison([.009]*12,[0]*12)['pass'])
        self.assertFalse(paired_comparison([.02]*8,[0]*8)['pass'])
        self.assertEqual(describe([1,2,3])['sd'],1.)

    def test_invalid_paired_shapes_and_nonfinite_rejected(self):
        with self.assertRaises(ValueError):paired_comparison([1,2],[1])
        with self.assertRaises(ValueError):paired_comparison([[1]],[[1]])
        with self.assertRaises(ValueError):describe([float('nan')])
        with self.assertRaises(ValueError):exact_sign_test([float('nan')])


if __name__=='__main__':unittest.main()
