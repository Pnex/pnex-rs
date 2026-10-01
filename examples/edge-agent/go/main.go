// Push values to the local PNeX edge agent (Go stdlib only): go run main.go
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"time"
)

const agent = "http://127.0.0.1:7070/v1/points"

type Point struct {
	Key    string `json:"key"`
	Value  any    `json:"value"`
	Unit   string `json:"unit,omitempty"`
	Ts     int64  `json:"ts,omitempty"`
	Record bool   `json:"record,omitempty"`
}

func push(points []Point) error {
	body, _ := json.Marshal(points)
	for attempt := 1; attempt <= 5; attempt++ {
		res, err := http.Post(agent, "application/json", bytes.NewReader(body))
		if err != nil {
			return err
		}
		res.Body.Close()
		switch res.StatusCode {
		case http.StatusAccepted:
			return nil
		case http.StatusServiceUnavailable: // backpressure: retry later
			time.Sleep(time.Duration(attempt) * time.Second)
		default:
			return fmt.Errorf("agent answered %s", res.Status)
		}
	}
	return fmt.Errorf("agent saturated")
}

func main() {
	now := time.Now().UnixMilli()
	err := push([]Point{
		{Key: "flow_rate", Value: 3.2, Unit: "m3/h", Ts: now},
		{Key: "pump_on", Value: true, Ts: now},
		{Key: "batch", Value: map[string]any{"id": 17, "recipe": "B"}, Record: true},
	})
	if err != nil {
		panic(err)
	}
	fmt.Println("ok")
}
