import unittest
import objects
class Objects(unittest.TestCase):
    def test_flattening_preserves_order_and_all_endpoint_values(self):
        for i in [0,1,29,121,224,299]:
            retained=objects.frame(i);flat=objects.frame(i,flattened=True)
            nodes={n['id']:n for n in retained['nodes']};camera=nodes[103]['transform'];lines=[]
            for child in nodes[103]['children']:
                node=nodes[child];transform=node['transform'];batch=nodes[node['children'][0]]
                for line in batch['segments']:
                    lines.append([line[0]+transform[4]+camera[4],line[1]+transform[5]+camera[5],line[2]+transform[4]+camera[4],line[3]+transform[5]+camera[5],*line[4:]])
            expected=next(n for n in flat['nodes'] if n['id']==103)['segments']
            self.assertEqual(lines,expected)
            self.assertEqual(len(lines),32*16)
    def test_shapes_are_retained_and_only_eight_objects_move(self):
        a={n['id']:n for n in objects.frame(0)['nodes']};b={n['id']:n for n in objects.frame(100)['nodes']}
        self.assertNotEqual(a[103],b[103])
        for i in range(32):
            self.assertEqual(a[300+i],b[300+i])
            if i>=8:self.assertEqual(a[200+i],b[200+i])
        self.assertEqual(objects.frame(225),objects.frame(299))
if __name__=='__main__':unittest.main()
