package main

import "fmt"

type P struct{ x, y, vx, vy int64 }

func step(p P) P {
	q := p
	q.x += q.vx
	q.y += q.vy
	q.vy += 1
	if q.x > 1000000 {
		q.vx = 0 - q.vx
	}
	if q.x < 0 {
		q.vx = 0 - q.vx
	}
	if q.y > 1000000 {
		q.vy = 0 - q.vy
	}
	if q.y < 0 {
		q.vy = 0 - q.vy
	}
	return q
}

func main() {
	var n int64 = 1000
	var ps []P
	var i int64
	for i < n {
		ps = append(ps, P{i * 997 % 1000000, i * 331 % 1000000, i%17 + 1, i%13 + 1})
		i++
	}
	for t := 0; t < 100000; t++ {
		for i = 0; i < n; i++ {
			ps[i] = step(ps[i])
		}
	}
	var sum int64
	for i = 0; i < n; i++ {
		p := ps[i]
		sum += p.x + p.y*3 + p.vx*5 + p.vy*7
	}
	fmt.Println(sum)
}
