package com.drawingtablet.input

import android.view.MotionEvent
import com.drawingtablet.network.InputEvent
import kotlin.math.cos
import kotlin.math.sin

/**
 * Captures stylus and touch input from MotionEvents.
 */
object InputCapture {

    /**
     * Convert a MotionEvent to InputEvents.
     *
     * @param event The motion event from the view
     * @param viewWidth Width of the view for normalization
     * @param viewHeight Height of the view for normalization
     * @return List of input events to send
     */
    fun processMotionEvent(
        event: MotionEvent,
        viewWidth: Int,
        viewHeight: Int
    ): List<InputEvent> {
        val events = mutableListOf<InputEvent>()

        when (event.getToolType(0)) {
            MotionEvent.TOOL_TYPE_STYLUS, MotionEvent.TOOL_TYPE_ERASER -> {
                events.addAll(processStylusEvent(event, viewWidth, viewHeight))
            }
            MotionEvent.TOOL_TYPE_FINGER -> {
                events.addAll(processTouchEvent(event, viewWidth, viewHeight))
            }
        }

        return events
    }

    private fun processStylusEvent(
        event: MotionEvent,
        viewWidth: Int,
        viewHeight: Int
    ): List<InputEvent> {
        val events = mutableListOf<InputEvent>()

        // Normalize coordinates to 0-1 range
        val x = event.x / viewWidth
        val y = event.y / viewHeight
        val pressure = event.pressure

        // Calculate tilt from orientation and tilt axes
        val tilt = event.getAxisValue(MotionEvent.AXIS_TILT)
        val orientation = event.getAxisValue(MotionEvent.AXIS_ORIENTATION)
        val tiltX = (Math.toDegrees(tilt.toDouble()) * cos(orientation.toDouble())).toFloat()
        val tiltY = (Math.toDegrees(tilt.toDouble()) * sin(orientation.toDouble())).toFloat()

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                events.add(InputEvent.StylusDown(x, y, pressure, tiltX, tiltY))
            }
            MotionEvent.ACTION_MOVE, MotionEvent.ACTION_HOVER_MOVE, MotionEvent.ACTION_HOVER_ENTER, MotionEvent.ACTION_HOVER_EXIT -> {
                // Process historical events for better accuracy (only for MOVE, not HOVER usually)
                if (event.actionMasked == MotionEvent.ACTION_MOVE) {
                    for (h in 0 until event.historySize) {
                        val hx = event.getHistoricalX(h) / viewWidth
                        val hy = event.getHistoricalY(h) / viewHeight
                        val hp = event.getHistoricalPressure(h)
                        events.add(InputEvent.StylusMove(hx, hy, hp, tiltX, tiltY))
                    }
                }
                events.add(InputEvent.StylusMove(x, y, pressure, tiltX, tiltY))
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_CANCEL -> {
                events.add(InputEvent.StylusUp)
            }
            MotionEvent.ACTION_BUTTON_PRESS -> {
                val button = when {
                    event.isButtonPressed(MotionEvent.BUTTON_STYLUS_PRIMARY) -> 0
                    event.isButtonPressed(MotionEvent.BUTTON_STYLUS_SECONDARY) -> 1
                    // Some Android devices report the stylus button as a generic secondary button (Right Click)
                    event.isButtonPressed(MotionEvent.BUTTON_SECONDARY) -> 0
                    else -> return events
                }
                events.add(InputEvent.StylusButton(button, true))
            }
            MotionEvent.ACTION_BUTTON_RELEASE -> {
                // Determine which button was released - release both to be safe as we don't track state here
                events.add(InputEvent.StylusButton(0, false))
                events.add(InputEvent.StylusButton(1, false))
            }
        }

        return events
    }

    private fun processTouchEvent(
        event: MotionEvent,
        viewWidth: Int,
        viewHeight: Int
    ): List<InputEvent> {
        val events = mutableListOf<InputEvent>()

        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val index = event.actionIndex
                val id = event.getPointerId(index)
                val x = event.getX(index) / viewWidth
                val y = event.getY(index) / viewHeight
                events.add(InputEvent.TouchDown(id, x, y))
            }
            MotionEvent.ACTION_MOVE -> {
                // Process historical events for smoother curves
                for (h in 0 until event.historySize) {
                    for (i in 0 until event.pointerCount) {
                        val id = event.getPointerId(i)
                        val x = event.getHistoricalX(i, h) / viewWidth
                        val y = event.getHistoricalY(i, h) / viewHeight
                        events.add(InputEvent.TouchMove(id, x, y))
                    }
                }

                // Current event
                for (i in 0 until event.pointerCount) {
                    val id = event.getPointerId(i)
                    val x = event.getX(i) / viewWidth
                    val y = event.getY(i) / viewHeight
                    events.add(InputEvent.TouchMove(id, x, y))
                }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP, MotionEvent.ACTION_CANCEL -> {
                val index = event.actionIndex
                val id = event.getPointerId(index)
                events.add(InputEvent.TouchUp(id))
            }
        }

        return events
    }
}
