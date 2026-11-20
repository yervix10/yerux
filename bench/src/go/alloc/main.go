package main

import "fmt"

var sinkL []int64 // global sinks force heap allocation (no stack promotion)
var sinkS []byte

func main() {
	var total, i int64
	for i < 6000000 {
		var l []int64
		for k := int64(0); k < 8; k++ {
			l = append(l, i+k)
		}
		s := append([]byte(nil), "item-"...)
		n := i
		var buf [24]byte
		c := 0
		for {
			buf[c] = byte('0' + n%10)
			c++
			n /= 10
			if n == 0 {
				break
			}
		}
		for c > 0 {
			c--
			s = append(s, buf[c])
		}
		sinkL, sinkS = l, s
		total = (total + l[7] + int64(len(l)) + int64(len(s)) + int64(s[5])) % 1000000007
		i++
	}
	fmt.Println(total)
}
