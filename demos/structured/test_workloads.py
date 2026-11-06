import unittest
import workloads
class Workloads(unittest.TestCase):
    def test_font_and_projection_bounds(self):
        self.assertEqual(len(workloads.FONT),45)
        for glyph in workloads.FONT.values():
            self.assertEqual(len(glyph.split('/')),7)
            self.assertTrue(all(len(row)==5 and set(row)<=set('01') for row in glyph.split('/')))
        for kind in ['editor','scroll','chart','diagram']:
            for sample in [0,17,100,299]:
                frame=workloads.frame(kind,sample)
                ids=[n['id'] for n in frame['nodes']]
                self.assertEqual(len(ids),len(set(ids)))
                for node in frame['nodes']:
                    self.assertTrue(set(node.get('children',[]))<=set(ids))
                    self.assertLessEqual(len(node.get('segments',[])),2048)
                    self.assertTrue(all(len(segment)==8 for segment in node.get('segments',[])))
    def test_idle_tail_and_retained_scroll_geometry(self):
        for kind in ['editor','scroll','chart','diagram']:
            self.assertEqual(workloads.frame(kind,225),workloads.frame(kind,299))
        a=workloads.frame('scroll',0);b=workloads.frame('scroll',100)
        for x,y in zip(a['nodes'],b['nodes']):
            if x['id']==103:self.assertNotEqual(x,y)
            else:self.assertEqual(x,y)
if __name__=='__main__':unittest.main()
