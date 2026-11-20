package main

import "fmt"

func main() {
	var n int64 = 300
	var a, b, c []int64
	var i int64
	for i = 0; i < n; i++ {
		for j := int64(0); j < n; j++ {
			a = append(a, (i*j+1)%10)
			b = append(b, (i+j)%10)
			c = append(c, 0)
		}
	}
	var grand int64
	for r := int64(0); r < 5; r++ {
		a[0] = r
		for i = 0; i < n; i++ {
			for j := int64(0); j < n; j++ {
				var acc int64
				for k := int64(0); k < n; k++ {
					acc += a[i*n+k] * b[k*n+j]
				}
				c[i*n+j] = acc
			}
		}
		for i = 0; i < n*n; i++ {
			grand += c[i] * (i % 7)
		}
	}
	fmt.Println(grand)
}
