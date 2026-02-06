package com.drawingtablet.input

import android.view.MotionEvent
import com.drawingtablet.network.InputEvent
import kotlin.math.cos
import kotlin.math.sin

/**
 * Captures stylus and touch input from MotionEvents.
 */
object InputCapture {
    private var lastButtonState = 0
    var isTouchEnabled: Boolean = true

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
                // Android has a native TOOL_TYPE_PALM (constant value 3), but it's not always reliable.
                // We check strictly for FINGER here, but we can also filter explicitly if needed.
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

        // Check for button state changes on every event
        val currentButtonState = event.buttonState
        if (currentButtonState != lastButtonState) {
            // Button 0 (Primary / Right Click)
            // Some devices map stylus button to BUTTON_SECONDARY (Right Click)
            val btn0Mask = MotionEvent.BUTTON_STYLUS_PRIMARY or MotionEvent.BUTTON_SECONDARY
            val wasPressed0 = (lastButtonState and btn0Mask) != 0
            val isPressed0 = (currentButtonState and btn0Mask) != 0
            
            if (wasPressed0 != isPressed0) {
                events.add(InputEvent.StylusButton(0, isPressed0))
            }

            // Button 1 (Secondary)
            val btn1Mask = MotionEvent.BUTTON_STYLUS_SECONDARY
            val wasPressed1 = (lastButtonState and btn1Mask) != 0
            val isPressed1 = (currentButtonState and btn1Mask) != 0

            if (wasPressed1 != isPressed1) {
                events.add(InputEvent.StylusButton(1, isPressed1))
            }

            lastButtonState = currentButtonState
        }

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
        }

        return events
    }

    private fun processTouchEvent(
        event: MotionEvent,
        viewWidth: Int,
        viewHeight: Int
    ): List<InputEvent> {
        // Mode 1: Touch Disabled (Pen Only)
        // Block all finger input if disabled
        if (!isTouchEnabled) {
            return emptyList()
        }

        // Native Palm Rejection Check
        // If the OS flags this event as a PALM (constant value 3), ignore it entirely.
        for (i in 0 until event.pointerCount) {
             if (event.getToolType(i) == 3) { // 3 = TOOL_TYPE_PALM
                 return emptyList()
             }
        }

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
