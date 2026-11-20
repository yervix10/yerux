package main

import "fmt"

func main() {
	var n int64 = 1000000
	var a, b []float64
	for i := int64(0); i < n; i++ {
		a = append(a, float64(i%100)*0.5)
		b = append(b, float64((i*7)%13)*0.25)
	}
	grand := 0.0
	for r := int64(0); r < 200; r++ {
		a[0] = float64(r)
		acc := 0.0
		for i := int64(0); i < n; i++ {
			acc = acc + a[i]*b[i]
		}
		grand = grand + acc*0.001
	}
	fmt.Println(int64(grand))
}
