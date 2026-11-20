package main

import (
	"fmt"
	"math"
)

func energy(x, y, z, vx, vy, vz, m []float64) float64 {
	e := 0.0
	for i := 0; i < 5; i++ {
		e = e + 0.5*m[i]*(vx[i]*vx[i]+vy[i]*vy[i]+vz[i]*vz[i])
		for j := i + 1; j < 5; j++ {
			dx, dy, dz := x[i]-x[j], y[i]-y[j], z[i]-z[j]
			e = e - m[i]*m[j]/math.Sqrt(dx*dx+dy*dy+dz*dz)
		}
	}
	return e
}

func main() {
	var x, y, z, vx, vy, vz, m []float64
	for k := 0; k < 5; k++ {
		f := float64(k)
		x = append(x, f*1.5-3.0)
		y = append(y, f*f*0.25-1.0)
		z = append(z, 2.0-f*0.75)
		vx = append(vx, 0.01*(f-2.0))
		vy = append(vy, 0.02*(1.0-f))
		vz = append(vz, 0.005*f)
		m = append(m, 1.0+f*0.5)
	}
	fmt.Println(int64(energy(x, y, z, vx, vy, vz, m) * 1000000000.0))
	dt := 0.001
	for s := int64(0); s < 2000000; s++ {
		for i := 0; i < 5; i++ {
			for j := i + 1; j < 5; j++ {
				dx, dy, dz := x[i]-x[j], y[i]-y[j], z[i]-z[j]
				d2 := dx*dx + dy*dy + dz*dz
				mag := dt / (d2 * math.Sqrt(d2))
				mi, mj := m[i]*mag, m[j]*mag
				vx[i] = vx[i] - dx*mj
				vy[i] = vy[i] - dy*mj
				vz[i] = vz[i] - dz*mj
				vx[j] = vx[j] + dx*mi
				vy[j] = vy[j] + dy*mi
				vz[j] = vz[j] + dz*mi
			}
		}
		for i := 0; i < 5; i++ {
			x[i] = x[i] + dt*vx[i]
			y[i] = y[i] + dt*vy[i]
			z[i] = z[i] + dt*vz[i]
		}
	}
	fmt.Println(int64(energy(x, y, z, vx, vy, vz, m) * 1000000000.0))
}
