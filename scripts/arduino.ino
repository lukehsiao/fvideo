// This Arduino script is used for latency measurements using a Arduino-based
// artificial saccade generator (ASG) as explained in
//
//     https://github.com/lukehsiao/eyelink-latency

// These pin numbers depend on which pins you use for your circuit
int sensorPin = A0;
int ledPin = 10;

void setup()
{
    // Use a 115200 baud rate
    Serial.begin(115200);
    pinMode(ledPin, OUTPUT);
    pinMode(sensorPin, INPUT);
}

void loop()
{
    static uint32_t ts1 = 0;
    static uint32_t ts2 = 0;
    static int state = 0;
    static int sensorValue = 0;
    int cmd = 0;

    switch (state) {
        case 0:
            digitalWrite(ledPin, HIGH);

            // poll for new command
            if (Serial.available() > 0) {
                cmd = Serial.read();

                // command from the host PC is hard-coded as the character 'g'
                if (cmd == 'g') {
                    // Start timer
                    ts1 = micros();

                    // Move to next state
                    state = 1;
                }
            }
            break;

        case 1:
            digitalWrite(ledPin, LOW);
            sensorValue = analogRead(sensorPin);

            // Rising edge trigger condition
            if (sensorValue > 512) {
                // Log time difference and send back to host PC
                ts2 = micros();
                Serial.println(ts2 - ts1);

                // Reset state
                state = 0;
                sensorValue = 0;
            }
            break;
    }
}
